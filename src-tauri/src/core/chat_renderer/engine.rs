use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use skia_safe::{
    surfaces, AlphaType, Color, ColorType, Font, FontMgr, FontStyle, ImageInfo, Paint, Rect,
    TextBlob,
};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::Hasher;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use stream_extractor::MessageSaved;
use tauri::{AppHandle, Emitter};

use crate::core::chat_renderer::args::{
    BackgroundMode, EvictionStrategy, QualityPreset, RenderVideoArgs,
    TimelineMismatchStrategy,
};
use crate::core::chat_renderer::emote_providers::{
    clear_token_cache, tokenise, EmoteNameMap, MessageToken, ResolvedEmote,
};
use crate::core::chat_renderer::helpers::{ease_out, get_user_color};
use crate::core::chat_renderer::types::{
    EmoteCache, EmoteData, ImageCache, LayoutLine, LayoutToken,
};
use crate::core::manager::AppTask;
use crate::error::AppError;
use crate::types::AppResult;

// ──────────────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────────────

const EMOTE_MARGIN: f32 = 6.0;

/// Base chunk size for frame batching. Each worker thread gets one unit of
/// work at a time, so this directly controls scheduling granularity.
/// Larger values mean more duplicate-frame coalescing before the rayon call
/// but longer latency before the first batch hits the IO thread.
const CHUNK_SIZE_BASE: usize = 16;

/// Bounded channel depth for the IO (FFmpeg) writer thread. A shallow channel
/// keeps peak RAM bounded while still absorbing small encode jitter.
const IO_CHANNEL_DEPTH: usize = 32;

/// Extra pool slots above `IO_CHANNEL_DEPTH` so in-flight rayon jobs can always
/// acquire a buffer without blocking the render loop.
const POOL_HEADROOM: usize = 8;

// ──────────────────────────────────────────────────────────────────────────────
// Thread-local state
// ──────────────────────────────────────────────────────────────────────────────

const MEASURE_CACHE_MAX: usize = 32_768;
type MeasureEntry = (f32, u32); // (width, generation)
const USER_COLOR_CACHE_MAX: usize = 512;

thread_local! {
    /// Per-thread Skia raster surface. Created lazily and reused across frames;
    /// reallocated only on canvas-size change (should never happen after init).
    static SKIA_SURFACE: RefCell<Option<skia_safe::Surface>> = RefCell::new(None);

    /// Per-thread text-measure cache: (hash of text + font size) → (width_px, gen).
    static PRE_RENDER_MEASURE_CACHE: RefCell<FxHashMap<u64, MeasureEntry>> =
        RefCell::new(FxHashMap::with_capacity_and_hasher(4096, Default::default()));

    /// Monotonic generation counter incremented each layout batch so old entries
    /// can be bulk-evicted instead of scanning the entire map.
    static MEASURE_GENERATION: RefCell<u32> = RefCell::new(0);

    /// Per-thread username → Color cache to avoid repeated hex parsing.
    static USER_COLOR_CACHE: RefCell<FxHashMap<u64, Color>> =
        RefCell::new(FxHashMap::with_capacity_and_hasher(64, Default::default()));

    // ── Pre-allocated Paint objects ──────────────────────────────────────────
    // Each Paint lives on the thread that draws frames. Reusing them across
    // draw_frame calls saves repeated construction / destruction overhead.

    static PAINT_BG: RefCell<Paint> = RefCell::new({
        let mut p = Paint::default(); p.set_anti_alias(true); p
    });
    static PAINT_HIGHLIGHT: RefCell<Paint> = RefCell::new({
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_style(skia_safe::paint::Style::Stroke);
        p.set_stroke_width(2.0);
        p
    });
    static PAINT_MASK_BG: RefCell<Paint> = RefCell::new({
        let mut p = Paint::default();
        p.set_anti_alias(true);
        p.set_color_filter(
            skia_safe::color_filters::blend(Color::WHITE, skia_safe::BlendMode::SrcIn),
        );
        p
    });
    static PAINT_TEXT: RefCell<Paint> = RefCell::new({
        let mut p = Paint::default(); p.set_anti_alias(true); p
    });
    static PAINT_EMOTE: RefCell<Paint> = RefCell::new(Paint::default());
    static PAINT_EMOTE_MASK: RefCell<Paint> = RefCell::new({
        let mut p = Paint::default();
        p.set_color_filter(
            skia_safe::color_filters::blend(Color::WHITE, skia_safe::BlendMode::SrcIn),
        );
        p
    });
}

// ──────────────────────────────────────────────────────────────────────────────
// Pixel buffer pool
// ──────────────────────────────────────────────────────────────────────────────

/// Lock-based pool of reusable pixel buffers. Avoids per-frame `malloc` /
/// `free` on the hot render path. The pool is bounded so it can't grow without
/// limit on high-core systems.
struct PixelBufferPool {
    inner: Mutex<Vec<Vec<u8>>>,
    max_buffers: usize,
}

impl PixelBufferPool {
    fn new(max_buffers: usize) -> Self {
        Self {
            inner: Mutex::new(Vec::with_capacity(max_buffers)),
            max_buffers,
        }
    }

    fn acquire(&self, min_len: usize) -> Vec<u8> {
        let mut guard = self.inner.lock().unwrap();
        if let Some(mut buf) = guard.pop() {
            drop(guard);
            // Only reallocate when the existing capacity is genuinely too small.
            // The old code called buf.reserve(min_len - buf.len()) with len==0,
            // which meant reserve(min_len) — triggering a realloc even when
            // capacity was already >= min_len.
            if buf.capacity() < min_len {
                buf.reserve_exact(min_len - buf.capacity());
            }
            // SAFETY: caller overwrites every byte (Skia read_pixels fills the
            // buffer completely). Content before set_len is don't-care.
            unsafe { buf.set_len(min_len) };
            return buf;
        }
        drop(guard);
        // Fresh allocation: allocate exactly the needed capacity in one step.
        let mut buf = Vec::with_capacity(min_len);
        // SAFETY: same as above.
        unsafe { buf.set_len(min_len) };
        buf
    }

    fn release(&self, mut buf: Vec<u8>) {
        // SAFETY: reset length to 0 so capacity is preserved but contents are
        // considered uninitialised, matching the contract of `acquire`.
        unsafe { buf.set_len(0) };
        let mut g = self.inner.lock().unwrap();
        if g.len() < self.max_buffers {
            g.push(buf);
        }
        // Otherwise the buffer is simply dropped here.
    }
}

/// RAII wrapper: returns the buffer to the pool on drop.
struct ReusableBuffer {
    pool: Arc<PixelBufferPool>,
    pub data: Option<Vec<u8>>,
}

impl Drop for ReusableBuffer {
    fn drop(&mut self) {
        if let Some(d) = self.data.take() {
            self.pool.release(d);
        }
    }
}

impl ReusableBuffer {
    fn new(pool: Arc<PixelBufferPool>, len: usize) -> Self {
        Self {
            data: Some(pool.acquire(len)),
            pool,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// ScheduledMessage
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct ScheduledMessage {
    spawn_frame: u32,
    /// Extra age (in frames) to add when computing how old this bubble is.
    ///
    /// For normal messages: 0 — age = (current_frame - spawn_frame) / fps.
    /// For prefill messages (injected at frame 0 because they were already
    /// on-screen when the video starts): the number of frames that elapsed
    /// *before* frame 0.  This makes hold/eviction/fade timers expire at
    /// exactly the right moment without needing negative spawn_frame values.
    age_offset_frames: u32,
    lines: Vec<LayoutLine>,
    bubble_w: i32,
    bubble_h: i32,
    bg_color: Color,
    user_color: Color,
    is_grouped: bool,
    has_animated_emotes: bool,
    is_highlighted: bool,
    /// Shortest animated-emote period (ms) in this message, or `None` if static.
    anim_period_ms: Option<u32>,
}

impl ScheduledMessage {
    fn new(
        spawn_frame: u32,
        lines: Vec<LayoutLine>,
        bubble_w: i32,
        bubble_h: i32,
        bg_color: Color,
        user_color: Color,
        is_grouped: bool,
        is_highlighted: bool,
    ) -> Self {
        Self::new_inner(spawn_frame, 0, lines, bubble_w, bubble_h, bg_color, user_color, is_grouped, is_highlighted)
    }

    /// Create a pre-filled message that is already mid-life at frame 0.
    ///
    /// `age_offset_frames` is the number of frames that elapsed before the
    /// video started (i.e. how old the message already is at frame 0).
    /// Slide and fade-in animations are suppressed — the message is settled.
    fn new_prefill(
        age_offset_frames: u32,
        lines: Vec<LayoutLine>,
        bubble_w: i32,
        bubble_h: i32,
        bg_color: Color,
        user_color: Color,
        is_grouped: bool,
        is_highlighted: bool,
    ) -> Self {
        // spawn_frame = 0: injected immediately at video start.
        // age_offset_frames carries the pre-existing age so timers work correctly.
        Self::new_inner(0, age_offset_frames, lines, bubble_w, bubble_h, bg_color, user_color, is_grouped, is_highlighted)
    }

    fn new_inner(
        spawn_frame: u32,
        age_offset_frames: u32,
        lines: Vec<LayoutLine>,
        bubble_w: i32,
        bubble_h: i32,
        bg_color: Color,
        user_color: Color,
        is_grouped: bool,
        is_highlighted: bool,
    ) -> Self {
        let mut has_animated_emotes = false;
        let mut anim_period_ms: Option<u32> = None;

        for l in &lines {
            for t in &l.tokens {
                if let LayoutToken::Emote { data, .. } = t {
                    if let EmoteData::Animated { total_ms, .. } = data.as_ref() {
                        has_animated_emotes = true;
                        anim_period_ms = Some(match anim_period_ms {
                            None => *total_ms,
                            Some(p) => p.min(*total_ms),
                        });
                    }
                }
            }
        }

        Self {
            spawn_frame,
            age_offset_frames,
            lines,
            bubble_w,
            bubble_h,
            bg_color,
            user_color,
            is_grouped,
            has_animated_emotes,
            is_highlighted,
            anim_period_ms,
        }
    }

    /// Effective age of this bubble at `frame_id`, in seconds.
    ///
    /// For normal messages: `(frame_id - spawn_frame) / fps`.
    /// For prefill messages: adds `age_offset_frames / fps` so the message
    /// appears to have been alive longer than it has been on screen.
    #[inline(always)]
    fn effective_age(&self, frame_id: u32, fps: f32) -> f32 {
        let on_screen_frames = frame_id.saturating_sub(self.spawn_frame);
        (on_screen_frames + self.age_offset_frames) as f32 / fps
    }

    /// Returns `true` when this bubble is in any animated state.
    #[inline(always)]
    fn is_animating(
        &self,
        frame_id: u32,
        fps: f32,
        anim_slide: bool,
        anim_fade: bool,
        eviction: &EvictionStrategy,
        hold_secs: f32,
    ) -> bool {
        let age = self.effective_age(frame_id, fps);
        // Prefill messages are never in the entrance animation window —
        // their effective age is already >= 0.5 s at frame 0 by construction
        // (we only prefill messages within hold_secs, which is >> 0.5 s).
        if (anim_slide || anim_fade) && age < 0.5 {
            return true;
        }
        if matches!(eviction, EvictionStrategy::Timed) && age > hold_secs {
            return true;
        }
        self.has_animated_emotes
    }

    /// Returns the GIF playback offset in ms at time `t_ms`.
    #[inline(always)]
    fn gif_frame_index_at(&self, t_ms: u64) -> u32 {
        match self.anim_period_ms {
            None | Some(0) => 0,
            Some(period) => (t_ms % period as u64) as u32,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Measure cache helpers
// ──────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn measure_key(s: &str, font_size_bits: u32) -> u64 {
    let mut h = FxHasher::default();
    h.write(s.as_bytes());
    h.write_u32(font_size_bits);
    h.finish()
}

/// Bulk-evict entries from a previous generation. Cheaper than an LRU on the
/// hot path because we only need generational granularity, not per-item recency.
fn evict_old_measure_entries(cache: &mut FxHashMap<u64, MeasureEntry>, current_gen: u32) {
    // Keep entries from the current or previous generation only.
    cache.retain(|_, (_, gen)| current_gen.saturating_sub(*gen) <= 1);
    // If still over capacity (very hot workload), keep only current gen.
    if cache.len() > MEASURE_CACHE_MAX {
        cache.retain(|_, (_, gen)| *gen == current_gen);
    }
}

#[inline(always)]
fn get_user_color_cached(username: &str, hex_color: &str) -> Color {
    // Hash both username and color so different colors for the same username
    // (e.g. after a color change) don't collide.
    let key = {
        let mut h = FxHasher::default();
        h.write(username.as_bytes());
        h.write_u8(0xFF);
        h.write(hex_color.as_bytes());
        h.finish()
    };
    USER_COLOR_CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some(&c) = cache.get(&key) {
            return c;
        }
        let color = get_user_color(username, hex_color);
        // Simple eviction: clear when at capacity. Works well because the
        // color distribution in a chat log is roughly uniform.
        if cache.len() >= USER_COLOR_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, color);
        color
    })
}

/// Measure a string with the given font, returning the advance width.
/// Result is cached per (text, font_size) pair within the current generation.
#[inline(always)]
fn measure_cached(
    font: &Font,
    font_bits: u32,
    s: &str,
    cache: &mut FxHashMap<u64, MeasureEntry>,
    gen: u32,
) -> f32 {
    let k = measure_key(s, font_bits);
    if let Some(&(w, _)) = cache.get(&k) {
        return w;
    }
    if cache.len() >= MEASURE_CACHE_MAX {
        evict_old_measure_entries(cache, gen);
    }
    let (w, _) = font.measure_str(s, None);
    cache.insert(k, (w, gen));
    w
}

/// Split `input` into substrings that each fit within `max_w` pixels.
/// Uses binary search over character offsets to minimise `measure_str` calls.
fn split_into_fragments<'a>(
    input: &'a str,
    font: &Font,
    font_bits: u32,
    max_w: f32,
    cache: &mut FxHashMap<u64, MeasureEntry>,
    gen: u32,
) -> arrayvec::ArrayVec<&'a str, 32> {
    let mut out = arrayvec::ArrayVec::new();
    let mut start = 0usize;

    while start < input.len() {
        let remainder = &input[start..];

        if measure_cached(font, font_bits, remainder, cache, gen) <= max_w {
            let _ = out.try_push(remainder);
            break;
        }

        // Collect all char-boundary byte offsets once — O(n) — so the binary
        // search below can index them in O(1) instead of re-walking from the
        // start on every iteration (which made the whole thing O(n log n)).
        let char_offsets: arrayvec::ArrayVec<usize, 256> = remainder
            .char_indices()
            .map(|(i, _)| i)
            .collect();
        let char_count = char_offsets.len();

        if char_count == 0 {
            break;
        }

        let mut lo = 1usize;
        let mut hi = char_count.saturating_sub(1).max(1);
        let mut best_byte = 0usize;

        while lo <= hi {
            let mid = (lo + hi) / 2;
            // O(1) lookup — just index into the pre-collected offsets.
            let byte_off = char_offsets.get(mid).copied().unwrap_or(remainder.len());

            if measure_cached(font, font_bits, &remainder[..byte_off], cache, gen) <= max_w {
                best_byte = byte_off.max(1);
                lo = mid + 1;
            } else {
                if mid == 0 {
                    break;
                }
                hi = mid - 1;
            }
        }

        if best_byte == 0 {
            // Even a single character doesn't fit — emit it to avoid an
            // infinite loop on very narrow canvases.
            let end = char_offsets.get(1).copied().unwrap_or(remainder.len());
            let _ = out.try_push(&remainder[..end]);
            start += end;
        } else {
            let _ = out.try_push(&remainder[..best_byte]);
            start += best_byte;
        }
    }

    out
}

// ──────────────────────────────────────────────────────────────────────────────
// layout_message_blocking
// ──────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn layout_message_blocking(
    content: &str,
    username: &str,
    user_hex_color: &str,
    username_font: &Font,
    message_font: &Font,
    available_w: f32,
    msg_line_h: f32,
    message_ascent: f32,
    emote_cache: &EmoteCache,
    image_cache: &ImageCache,
    args: &RenderVideoArgs,
    emote_map: &EmoteNameMap,
    measure_cache: &mut FxHashMap<u64, MeasureEntry>,
    gen: u32,
    is_grouped: bool,
) -> Result<(Vec<LayoutLine>, i32, i32, Color), AppError> {
    let uf_bits = username_font.size().to_bits();
    let mf_bits = message_font.size().to_bits();

    let parsed_user_color = get_user_color_cached(username, user_hex_color);

    // Build the emote-map option tuple respecting per-provider flags.
    let flags = &args.emote_providers;
    let map_opt = if !emote_map.is_empty() && flags.any_name_provider_enabled() {
        Some((emote_map, flags))
    } else {
        None
    };
    let tokens = tokenise(content, map_opt);
    let max_w = available_w.max(1.0);

    // Build the username prefix inline — avoids a heap allocation for the
    // common case where the username is short.
    let prefix_owned;
    let prefix_str: &str = if username.len() <= 94 {
        use std::fmt::Write as _;
        let mut buf = arrayvec::ArrayString::<96>::new();
        let _ = buf.write_str(username);
        let _ = buf.write_str(": ");
        prefix_owned = buf;
        &prefix_owned
    } else {
        "[long username]: "
    };

    let prefix_w = measure_cached(username_font, uf_bits, prefix_str, measure_cache, gen);
    let space_w = measure_cached(message_font, mf_bits, " ", measure_cache, gen);

    // ── Word-wrap pass ────────────────────────────────────────────────────────
    let mut lines: Vec<Vec<MessageToken>> = Vec::with_capacity(4);
    let mut current_line: Vec<MessageToken> = Vec::with_capacity(16);
    let mut cur_w = if is_grouped { 0.0 } else { prefix_w };
    let mut last_was_zero_width = false;

    for token in &tokens {
        match token {
            MessageToken::Text(s) => {
                // Fast path: whitespace-only token (e.g. the " " emitted by
                // push_text_segment between words). split_ascii_whitespace on
                // a space yields nothing, silently dropping the space and
                // merging the surrounding words. Handle it directly instead.
                if s.chars().all(|c| c.is_whitespace()) {
                    if !current_line.is_empty() {
                        // Only insert a space if we're not at the very start
                        // of a new line and there's room for at least one more
                        // character. The space advances x_cursor in the bake
                        // pass via measure_cached even without a Skia blob
                        // (TextBlob::from_str returns None for whitespace-only
                        // strings, but x_cursor still advances by space_w).
                        current_line.push(MessageToken::Text(" "));
                        cur_w += space_w;
                        last_was_zero_width = false;
                    }
                    continue;
                }
                for (pi, para) in s.split('\n').enumerate() {
                    if para.is_empty() && pi > 0 {
                        lines.push(std::mem::take(&mut current_line));
                        cur_w = 0.0;
                        continue;
                    }
                    let mut first_word = true;
                    for raw_word in para.split_ascii_whitespace() {
                        let word_w =
                            measure_cached(message_font, mf_bits, raw_word, measure_cache, gen);
                        let needed_space = if first_word { 0.0 } else { space_w };

                        if cur_w + needed_space + word_w <= max_w {
                            if !first_word {
                                current_line.push(MessageToken::Text(" "));
                                cur_w += space_w;
                            }
                            current_line.push(MessageToken::Text(raw_word));
                            cur_w += word_w;
                        } else {
                            if !current_line.is_empty() {
                                lines.push(std::mem::take(&mut current_line));
                                cur_w = 0.0;
                            }
                            if word_w > max_w {
                                let frags = split_into_fragments(
                                    raw_word,
                                    message_font,
                                    mf_bits,
                                    max_w,
                                    measure_cache,
                                    gen,
                                );
                                let flen = frags.len();
                                for (fi, f) in frags.into_iter().enumerate() {
                                    current_line.push(MessageToken::Text(f));
                                    cur_w += measure_cached(
                                        message_font,
                                        mf_bits,
                                        f,
                                        measure_cache,
                                        gen,
                                    );
                                    if fi < flen - 1 {
                                        lines.push(std::mem::take(&mut current_line));
                                        cur_w = 0.0;
                                    }
                                }
                            } else {
                                current_line.push(MessageToken::Text(raw_word));
                                cur_w = word_w;
                            }
                        }
                        first_word = false;
                        last_was_zero_width = false;
                    }
                }
            }
            MessageToken::KickEmote { id } => {
                if !flags.kick {
                    continue;
                }
                let parsed_id = id.parse::<i32>().unwrap_or(0);
                let ew = emote_cache
                    .get(parsed_id)
                    .map(|ed| ed.width() as f32)
                    .unwrap_or(emote_cache.target_height() as f32);
                let padded = ew + EMOTE_MARGIN;
                if cur_w + padded > max_w && !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                    cur_w = 0.0;
                }
                current_line.push(token.clone());
                cur_w += padded;
                last_was_zero_width = false;
            }
            MessageToken::ProviderEmote(ResolvedEmote {
                                            url, zero_width, ..
                                        }) => {
                let mw = image_cache
                    .get(url)
                    .map(|ed| ed.width() as f32)
                    .unwrap_or(image_cache.target_height() as f32);
                if *zero_width && !current_line.is_empty() && !last_was_zero_width {
                    current_line.push(token.clone());
                    last_was_zero_width = true;
                    continue;
                }
                let padded = mw + EMOTE_MARGIN;
                if cur_w + padded > max_w && !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                    cur_w = 0.0;
                }
                current_line.push(token.clone());
                cur_w += padded;
                last_was_zero_width = false;
            }
            MessageToken::ImageUrl(url) => {
                if !flags.image_urls {
                    continue;
                }
                let mw = image_cache
                    .get(url)
                    .map(|ed| ed.width() as f32)
                    .unwrap_or(image_cache.target_height() as f32);
                let padded = mw + EMOTE_MARGIN;
                if cur_w + padded > max_w && !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                    cur_w = 0.0;
                }
                current_line.push(token.clone());
                cur_w += padded;
                last_was_zero_width = false;
            }
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    // ── Measure + bake TextBlobs ───────────────────────────────────────────────
    let bubble_pad = args.bubble_padding.max(0) as f32;
    let mut layout_lines = Vec::with_capacity(lines.len());
    let mut measured_max_w = 0f32;
    let mut y_cursor = bubble_pad;

    for (li, line) in lines.iter().enumerate() {
        // Determine line height from the tallest token.
        let mut lh = msg_line_h;
        for token in line {
            match token {
                MessageToken::Text(_) => {}
                MessageToken::KickEmote { id } => {
                    let pid = id.parse::<i32>().unwrap_or(0);
                    let h = emote_cache
                        .get(pid)
                        .map(|ed| ed.height() as f32)
                        .unwrap_or(emote_cache.target_height() as f32);
                    lh = lh.max(h);
                }
                MessageToken::ProviderEmote(ResolvedEmote {
                                                url, zero_width, ..
                                            }) => {
                    let h = image_cache
                        .get(url)
                        .map(|ed| ed.height() as f32)
                        .unwrap_or(image_cache.target_height() as f32);
                    if !zero_width {
                        lh = lh.max(h + 8.0);
                    }
                }
                MessageToken::ImageUrl(url) => {
                    let h = image_cache
                        .get(url)
                        .map(|ed| ed.height() as f32)
                        .unwrap_or(image_cache.target_height() as f32);
                    lh = lh.max(h + 8.0);
                }
            }
        }

        // Baseline is the bottom of text glyphs within the (potentially taller)
        // line box. Negative `message_ascent` means we subtract it.
        let baseline = y_cursor + ((lh - msg_line_h) / 2.0).max(0.0) - message_ascent;
        let mut x_cursor = bubble_pad;
        let mut layout_tokens: Vec<LayoutToken> = Vec::with_capacity(line.len() + 1);

        // First line only: prefix glyph (unless grouped).
        if li == 0 && !is_grouped {
            if let Some(blob) = TextBlob::from_str(prefix_str, username_font) {
                layout_tokens.push(LayoutToken::Glyph {
                    blob,
                    x: x_cursor,
                    y: baseline,
                    width: prefix_w,
                });
            }
            x_cursor += prefix_w;
        }

        for token in line {
            match token {
                MessageToken::Text(s) => {
                    let w = measure_cached(message_font, mf_bits, s, measure_cache, gen);
                    if let Some(blob) = TextBlob::from_str(s, message_font) {
                        layout_tokens.push(LayoutToken::Glyph {
                            blob,
                            x: x_cursor,
                            y: baseline,
                            width: w,
                        });
                    }
                    x_cursor += w;
                }
                MessageToken::KickEmote { id } => {
                    let pid = id.parse::<i32>().unwrap_or(0);
                    // Always advance x_cursor by the same width used in the
                    // word-wrap pass so subsequent tokens are not shifted left
                    // when the emote is missing from cache.
                    let fallback_w = emote_cache.target_height() as f32;
                    if let Some(ed) = emote_cache.get(pid) {
                        let ew = ed.width() as f32;
                        let draw_y = if args.center_emotes_vertically {
                            y_cursor + (lh - ed.height() as f32) / 2.0
                        } else {
                            y_cursor
                        };
                        layout_tokens.push(LayoutToken::Emote {
                            data: ed,
                            x: x_cursor + (EMOTE_MARGIN / 2.0),
                            y: draw_y,
                        });
                        x_cursor += ew + EMOTE_MARGIN;
                    } else {
                        // Emote not in cache (evicted or failed): reserve its
                        // space so the rest of the line stays correctly aligned.
                        x_cursor += fallback_w + EMOTE_MARGIN;
                    }
                }
                MessageToken::ProviderEmote(ResolvedEmote {
                                                url, zero_width, ..
                                            }) => {
                    let fallback_w = image_cache.target_height() as f32;
                    if let Some(ed) = image_cache.get(url) {
                        let sw = ed.width() as f32;
                        let target_x = if *zero_width && x_cursor > bubble_pad {
                            x_cursor - sw - (EMOTE_MARGIN / 2.0)
                        } else {
                            x_cursor + (EMOTE_MARGIN / 2.0)
                        };
                        let draw_y = if args.center_emotes_vertically {
                            y_cursor + (lh - ed.height() as f32) / 2.0
                        } else {
                            y_cursor
                        };
                        layout_tokens.push(LayoutToken::Emote {
                            data: ed,
                            x: target_x,
                            y: draw_y,
                        });
                        if !zero_width {
                            x_cursor += sw + EMOTE_MARGIN;
                        }
                    } else if !zero_width {
                        x_cursor += fallback_w + EMOTE_MARGIN;
                    }
                }
                MessageToken::ImageUrl(url) => {
                    let fallback_w = image_cache.target_height() as f32;
                    if let Some(ed) = image_cache.get(url) {
                        let iw = ed.width() as f32;
                        let draw_y = if args.center_emotes_vertically {
                            y_cursor + (lh - ed.height() as f32) / 2.0
                        } else {
                            y_cursor
                        };
                        layout_tokens.push(LayoutToken::Emote {
                            data: ed,
                            x: x_cursor + (EMOTE_MARGIN / 2.0),
                            y: draw_y,
                        });
                        x_cursor += iw + EMOTE_MARGIN;
                    } else {
                        x_cursor += fallback_w + EMOTE_MARGIN;
                    }
                }
            }
        }

        measured_max_w = measured_max_w.max(x_cursor - bubble_pad);
        layout_lines.push(LayoutLine {
            tokens: layout_tokens,
            line_height: lh,
            total_width: x_cursor - bubble_pad,
        });
        y_cursor += lh;
    }

    let content_width = (measured_max_w + bubble_pad * 2.0).ceil() as i32;
    let final_width = if args.bubble_mode_full_width {
        (max_w.ceil() as i32).max(1)
    } else {
        content_width.max(1)
    };
    let final_height = (y_cursor + bubble_pad).ceil() as i32;

    Ok((layout_lines, final_width, final_height, parsed_user_color))
}

// ──────────────────────────────────────────────────────────────────────────────
// Core frame draw
// ──────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_frame(
    canvas: &skia_safe::Canvas,
    bubbles: &[Arc<ScheduledMessage>],
    args: &RenderVideoArgs,
    bg_color: Color,
    is_luma: bool,
    frame_id: u32,
    fps_f32: f32,
    hold_secs: f32,
    anim_slide: bool,
    anim_fade: bool,
    eviction: &EvictionStrategy,
) {
    canvas.clear(bg_color);

    if bubbles.is_empty() {
        return;
    }

    let t_ms = ((frame_id as f64 * 1000.0) / args.fps as f64) as u64;
    let fade_out_f = args.message_fade_out_seconds as f32;
    let msg_color = Color::from(&args.message_color);
    let hi_color = Color::from(&args.highlight_color);
    let outline_w = args.username_outline_width.unwrap_or(1.5);

    let mut y_cursor = (args.height - args.padding) as f32;

    // Acquire all paint borrows before the bubble loop to avoid repeated
    // RefCell overhead per bubble.
    PAINT_BG.with(|pb| {
        PAINT_HIGHLIGHT.with(|ph| {
            PAINT_MASK_BG.with(|pm| {
                PAINT_TEXT.with(|pt| {
                    PAINT_EMOTE.with(|pe| {
                        PAINT_EMOTE_MASK.with(|pem| {
                            let mut paint_bg = pb.borrow_mut();
                            let mut paint_highlight = ph.borrow_mut();
                            let mut mask_bg = pm.borrow_mut();
                            let mut text_paint = pt.borrow_mut();
                            let mut emote_paint = pe.borrow_mut();
                            let mut emote_mask_paint = pem.borrow_mut();

                            for bubble in bubbles {
                                if y_cursor < 0.0 {
                                    break;
                                }

                                let age_secs = bubble.effective_age(frame_id, fps_f32);

                                // Compute per-bubble alpha — combined fade-in and fade-out.
                                let alpha = {
                                    let mut a = 1.0f32;
                                    if anim_fade && age_secs < 0.5 {
                                        a *= age_secs / 0.5;
                                    }
                                    if matches!(eviction, EvictionStrategy::Timed)
                                        && age_secs > hold_secs
                                    {
                                        a *= 1.0
                                            - ((age_secs - hold_secs) / fade_out_f).clamp(0.0, 1.0);
                                    }
                                    a
                                };

                                // Skip invisible bubbles entirely — no state mutations needed.
                                if alpha <= 0.0 {
                                    y_cursor -=
                                        bubble.bubble_h as f32 + args.message_spacing as f32;
                                    continue;
                                }

                                let byte_alpha = (255.0 * alpha) as u8;
                                let top = y_cursor - bubble.bubble_h as f32;
                                let final_x = args.padding as f32;

                                let x_translate = if anim_slide && age_secs < 0.5 {
                                    final_x
                                        + (1.0 - ease_out(age_secs / 0.5))
                                        * (args.width as f32 - final_x)
                                } else {
                                    final_x
                                };

                                // ── Colour pass ─────────────────────────────────────────────────
                                canvas.save();
                                canvas.translate((x_translate, top));

                                paint_bg.set_color(bubble.bg_color.with_a(byte_alpha));
                                let rect = Rect::new(
                                    0.0,
                                    0.0,
                                    bubble.bubble_w as f32,
                                    bubble.bubble_h as f32,
                                );
                                canvas.draw_round_rect(
                                    rect,
                                    args.bubble_radius,
                                    args.bubble_radius,
                                    &paint_bg,
                                );

                                if bubble.is_highlighted {
                                    paint_highlight.set_color(hi_color.with_a(byte_alpha));
                                    canvas.draw_round_rect(
                                        rect,
                                        args.bubble_radius,
                                        args.bubble_radius,
                                        &paint_highlight,
                                    );
                                }

                                draw_bubble_tokens(
                                    canvas,
                                    bubble,
                                    &mut text_paint,
                                    &mut emote_paint,
                                    t_ms,
                                    alpha,
                                    byte_alpha,
                                    msg_color,
                                    args,
                                    outline_w,
                                    false,
                                );

                                canvas.restore();

                                // ── Mask pass (luma matte only) ─────────────────────────────────
                                if is_luma {
                                    let mask_x = x_translate + args.width as f32;
                                    canvas.save();
                                    canvas.translate((mask_x, top));

                                    mask_bg.set_color(Color::WHITE.with_a(byte_alpha));
                                    canvas.draw_round_rect(
                                        rect,
                                        args.bubble_radius,
                                        args.bubble_radius,
                                        &mask_bg,
                                    );

                                    draw_bubble_tokens(
                                        canvas,
                                        bubble,
                                        &mut text_paint,
                                        &mut emote_mask_paint,
                                        t_ms,
                                        alpha,
                                        byte_alpha,
                                        Color::WHITE,
                                        args,
                                        outline_w,
                                        true,
                                    );

                                    canvas.restore();
                                }

                                y_cursor -= bubble.bubble_h as f32 + args.message_spacing as f32;
                            }
                        })
                    })
                })
            })
        })
    });
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn draw_bubble_tokens(
    canvas: &skia_safe::Canvas,
    bubble: &ScheduledMessage,
    text_paint: &mut Paint,
    emote_paint: &mut Paint,
    t_ms: u64,
    alpha: f32,
    byte_alpha: u8,
    msg_color: Color,
    args: &RenderVideoArgs,
    outline_w: f32,
    is_mask: bool,
) {
    for (li, line) in bubble.lines.iter().enumerate() {
        for (ti, token) in line.tokens.iter().enumerate() {
            match token {
                LayoutToken::Glyph { blob, x, y, .. } => {
                    let is_username = !bubble.is_grouped && li == 0 && ti == 0;

                    if is_mask {
                        text_paint.set_color(Color::from_argb(byte_alpha, 255, 255, 255));
                        canvas.draw_text_blob(blob, (*x, *y), text_paint);
                    } else {
                        let base_color = if is_username {
                            bubble.user_color
                        } else {
                            msg_color
                        };
                        let final_color =
                            base_color.with_a((base_color.a() as f32 * alpha).min(255.0) as u8);

                        // Only draw expensive decorations when visible enough to matter.
                        if is_username && byte_alpha > 5 {
                            if args.username_shadow {
                                text_paint.set_color(Color::from_argb(
                                    (180.0 * alpha) as u8,
                                    0,
                                    0,
                                    0,
                                ));
                                canvas.draw_text_blob(blob, (*x + 2.0, *y + 2.0), text_paint);
                            }
                            if args.outline_usernames {
                                text_paint.set_style(skia_safe::paint::Style::Stroke);
                                text_paint.set_stroke_width(outline_w);
                                text_paint.set_color(Color::from_argb(
                                    (200.0 * alpha) as u8,
                                    0,
                                    0,
                                    0,
                                ));
                                canvas.draw_text_blob(blob, (*x, *y), text_paint);
                                text_paint.set_style(skia_safe::paint::Style::Fill);
                            }
                        }

                        text_paint.set_color(final_color);
                        canvas.draw_text_blob(blob, (*x, *y), text_paint);
                    }
                }
                LayoutToken::Emote { data, x, y } => {
                    if let Some(img) = data.frame_at(t_ms) {
                        let dest =
                            Rect::new(*x, *y, *x + data.width() as f32, *y + data.height() as f32);
                        // In the mask pass, emote_paint is PAINT_EMOTE_MASK which carries a
                        // SrcIn white color-filter. set_alpha respects fade-in/out while the
                        // filter ensures the emote renders as a white silhouette so the luma
                        // matte has the correct shape instead of the raw image colors (which
                        // produced black circles on the alpha channel).
                        emote_paint.set_alpha(byte_alpha);
                        canvas.draw_image_rect(img, None, dest, emote_paint);
                    }
                }
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Dirty-frame signature
// ──────────────────────────────────────────────────────────────────────────────

/// Produce a hash that changes exactly when the visual output changes.
///
/// Alpha and slide-offset values are bucketed into 64 discrete steps instead
/// of the full u8 range so that slow animations produce long runs of identical
/// signatures, allowing many consecutive frames to be coalesced into a single
/// render call.
#[allow(clippy::too_many_arguments)]
#[inline]
/// Produces a hash that changes exactly when the visual output changes.
/// Operates directly on `&VecDeque` so the render loop avoids cloning
/// `active_bubbles` just to compute a signature.
///
/// Alpha and slide-offset values are bucketed into 64 discrete steps so slow
/// animations produce long runs of identical signatures, coalescing many
/// consecutive frames into a single render call.
fn frame_signature_deque(
    bubbles: &VecDeque<Arc<ScheduledMessage>>,
    frame_id: u32,
    fps_f32: f32,
    anim_slide: bool,
    anim_fade: bool,
    eviction: &EvictionStrategy,
    hold_secs: f32,
    fade_secs: f32,
) -> u64 {
    let t_ms = ((frame_id as f64 * 1000.0) / fps_f32 as f64) as u64;
    let mut h = FxHasher::default();
    h.write_usize(bubbles.len());
    for b in bubbles {
        h.write_u32(b.spawn_frame);
        h.write_u32(b.age_offset_frames);
        let age = b.effective_age(frame_id, fps_f32);
        let mut a = 1.0f32;
        // Match draw_frame exactly: either slide OR fade triggers the 0.5s
        // fade-in ramp. If only anim_slide was on, the old code would produce
        // a signature that never changed during the slide, causing the frame
        // to be deduped despite the bubble visually moving.
        if (anim_slide || anim_fade) && age < 0.5 {
            a = age / 0.5;
        }
        if matches!(eviction, EvictionStrategy::Timed) && age > hold_secs {
            a = 1.0 - ((age - hold_secs) / fade_secs).clamp(0.0, 1.0);
        }
        let alpha_bucket = (a * 63.0) as u8;
        h.write_u8(alpha_bucket);
        if anim_slide && age < 0.5 {
            let offset_bucket = (ease_out(age / 0.5) * 63.0) as u8;
            h.write_u8(offset_bucket);
        }
        if b.has_animated_emotes {
            h.write_u32(b.gif_frame_index_at(t_ms));
        }
    }
    h.finish()
}

// ──────────────────────────────────────────────────────────────────────────────
// FFmpeg argument builders
// ──────────────────────────────────────────────────────────────────────────────

/// Probe the duration of a video file using ffprobe and return the number of
/// frames at the given fps.
///
/// This is a fast, read-only metadata query — ffprobe reads only the container
/// header, typically finishing in under 100 ms even for large files.  We use
/// it to cap `total_frames` in overlay mode so the render loop never generates
/// more frames than the base video actually needs.
///
/// Returns `None` if ffprobe is not available or the file cannot be read.
fn probe_video_frames(path: &str, fps: u32) -> Option<u32> {
    // Helper: run ffprobe with the given show_entries arg and return the
    // first non-empty line of stdout as an f64.
    let try_probe = |show_entries: &str| -> Option<f64> {
        let out = Command::new("ffprobe")
            .args([
                "-v", "error",
                "-select_streams", "v:0",
                "-show_entries", show_entries,
                "-of", "default=noprint_wrappers=1:nokey=1",
                path,
            ])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let line = s.lines().find(|l| !l.trim().is_empty())?;
        line.trim().parse().ok()
    };

    // Try stream-level duration first (most formats expose this).
    // Fall back to format/container duration — some containers (MKV, MPEG-TS)
    // store duration only at the container level, not per-stream.
    let duration_secs = try_probe("stream=duration")
        .or_else(|| try_probe("format=duration"))?;

    // Add a small buffer so the very last frame is never clipped by
    // floating-point rounding or VFR duration imprecision.
    let frames = ((duration_secs + 3.0) * fps as f64).ceil() as u32;
    Some(frames)
}

/// Probe whether FFmpeg was compiled with h264_nvenc support.
///
/// We test for NVENC here purely for *encoding* speed — the GPU is never used
/// for pixel rendering or compositing. All rasterisation happens on CPU via
/// Skia. NVENC only handles the final H.264 encode step and is optional; the
/// pipeline works identically (just slower on encode) without it.
///
/// Note: the software fallback (`libx264`) is typically not the bottleneck.
/// At 400×800 @ 24 fps the render + compositing phase dominates. If you need
/// the absolute fastest encode, consider lowering `fps` or `quality_preset`
/// rather than relying on NVENC.
fn probe_nvenc() -> bool {
    Command::new("ffmpeg")
        .args(["-h", "encoder=h264_nvenc"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("h264_nvenc"))
        .unwrap_or(false)
}

/// Build FFmpeg arguments for the overlay pipeline (base video + chat stream).
///
/// # Direct overlay pipeline
///
/// ```text
/// ┌──────────────────────────────────────────────────────────────────────┐
/// │  [base video file]  →─── input 0 ──────────────────────────────┐    │
/// │                                                                  ↓    │
/// │  [raw BGRA frames]  →─── input 1 (stdin) → filter_complex → encode → output │
/// │   (chat renderer)                         [overlay filter]           │
/// └──────────────────────────────────────────────────────────────────────┘
/// ```
///
/// For `LumaMatte` mode the luma split happens entirely inside FFmpeg's
/// `filter_complex` — no intermediate file or extra process is needed. The
/// chat canvas is already double-width (colour left, alpha-mask right); FFmpeg
/// crops and alphamerges them before overlaying onto the base video.
///
/// For `Transparent` mode the raw BGRA frames already carry full alpha;
/// FFmpeg composites with `alpha=premultiplied` directly.
///
/// Audio is always copied verbatim from the base video (`-c:a copy`).
fn build_overlay_ffmpeg_args(
    args: &RenderVideoArgs,
    actual_width: i32,
    is_luma: bool,
    has_nvenc: bool,
    ffmpeg_preset: &str,
) -> Vec<String> {
    let mut a = vec!["-y".to_string()];

    if has_nvenc {
        a.extend(["-hwaccel".into(), "auto".into()]);
    }

    let base_video = args.overlay_video_path.as_ref().unwrap();

    // Input 0: base video
    a.extend([
        "-thread_queue_size".into(),
        "4096".into(),
        "-i".into(),
        base_video.clone(),
    ]);

    // Input 1: raw BGRA chat frames from stdin
    a.extend([
        "-thread_queue_size".into(),
        "4096".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "bgra".into(),
        "-s".into(),
        format!("{}x{}", actual_width, args.height),
        "-r".into(),
        args.fps.to_string(),
        "-i".into(),
        "-".into(),
    ]);

    // Extra inputs for image overlays: each file is a separate -i input so
    // FFmpeg decodes it once and caches the frames — zero extra per-frame CPU
    // cost. Shapes are synthesised as `color` filter sources inside the
    // filter_complex — no separate -i needed.
    //
    // Image overlay inputs start at index 2.
    let img_input_start = 2usize;
    for ov in &args.image_overlays {
        a.extend([
            "-thread_queue_size".into(),
            "512".into(),
            "-loop".into(),
            "1".into(),        // loop static image for the full duration
            "-i".into(),
            ov.asset_path.clone(),
        ]);
    }

    let ox = args.overlay_x.unwrap_or(0);
    let oy = args.overlay_y.unwrap_or(0);
    let eof_action = match args.timeline_mismatch_strategy {
        TimelineMismatchStrategy::RenderClearCanvas => "eof_action=pass",
        _ => "eof_action=repeat",
    };

    // ── Build filter_complex ─────────────────────────────────────────────────
    //
    // Layer order (bottom to top):
    //   [0:v] base video
    //   → shape overlays  (color filter sources, composited in order)
    //   → image overlays  (movie / -i inputs, composited in order)
    //   → chat overlay    (the BGRA stream from stdin, with alpha)
    //
    // Each step tags its output [base_N] so the next step can reference it.
    // All overlay positions are absolute video-pixel coordinates, matching
    // exactly what the user specified in CustomShapeOverlay.x / .y.
    let mut filter_parts: Vec<String> = Vec::new();

    // Running label for the "current base" as we stack layers.
    let mut current_base = "[0:v]".to_string();
    let mut label_idx = 0usize;

    // ── Shape overlays ────────────────────────────────────────────────────────
    // Each shape is a solid-color rectangle synthesised with the `color` filter
    // and composited at (shape.x, shape.y) with the shape's alpha.
    for shape in &args.shape_overlays {
        let next_label = format!("[base{}]", label_idx);
        label_idx += 1;

        // ObjectColor fields are named red/green/blue/alpha (i32, 0–255).
        let r = shape.color.red.clamp(0, 255) as u8;
        let g = shape.color.green.clamp(0, 255) as u8;
        let b = shape.color.blue.clamp(0, 255) as u8;
        let alpha_f = shape.color.alpha.clamp(0, 255) as f32 / 255.0;

        // color=c=0xRRGGBB:s=WxH generates a constant-color source.
        // scale2ref sizes it to shape.width × shape.height before overlay.
        // `enable='between(t,0,9999)'` keeps it alive for the full video.
        // The overlay filter's `alpha` option handles straight alpha blending
        // (FFmpeg's overlay works on packed formats; format=yuva420p gives us
        //  per-pixel alpha so shape corners with corner_radius can be handled
        //  — but since shapes are rectangles here we just use opacity).
        //
        // Note: corner_radius is not supported natively in FFmpeg's color
        // filter; rounded-rect shapes require drawbox (no radius) or a
        // generated PNG.  For simplicity we render axis-aligned rectangles
        // exactly as specified; corner_radius is preserved in the struct for
        // Skia use if standalone mode ever draws shapes again.
        let shape_filter = format!(
            "color=c=0x{:02X}{:02X}{:02X}@{:.4}:s={}x{}:r={},setpts=PTS-STARTPTS[shape{}]; \
             {}[shape{}]overlay={}:{}:format=auto{}",
            r, g, b, alpha_f,
            shape.width as u32, shape.height as u32, args.fps,
            label_idx - 1,
            current_base, label_idx - 1,
            shape.x as i32, shape.y as i32,
            if shape.corner_radius > 0.0 { "" } else { "" }, // no-op; kept for clarity
        );
        // Append ":shortest=1" so a missing video end doesn't stall the mux.
        let shape_filter = format!("{}{}", shape_filter, next_label);
        filter_parts.push(shape_filter);
        current_base = next_label;
    }

    // ── Image overlays ────────────────────────────────────────────────────────
    // Each image was added as a -i input above; reference it by index.
    for (i, ov) in args.image_overlays.iter().enumerate() {
        let input_idx = img_input_start + i;
        let next_label = format!("[base{}]", label_idx);
        label_idx += 1;

        let alpha_f = ov.alpha.clamp(0.0, 1.0);
        let img_label = format!("[img{}]", i);

        // Scale to requested dimensions if specified; otherwise native size.
        let scale_filter = match (ov.width, ov.height) {
            (Some(w), Some(h)) => format!(
                "[{}:v]scale={}:{}:flags=lanczos,setpts=PTS-STARTPTS,format=rgba,colorchannelmixer=aa={:.4}{}",
                input_idx, w as u32, h as u32, alpha_f, img_label
            ),
            (Some(w), None) => format!(
                "[{}:v]scale={}:-1:flags=lanczos,setpts=PTS-STARTPTS,format=rgba,colorchannelmixer=aa={:.4}{}",
                input_idx, w as u32, alpha_f, img_label
            ),
            (None, Some(h)) => format!(
                "[{}:v]scale=-1:{}:flags=lanczos,setpts=PTS-STARTPTS,format=rgba,colorchannelmixer=aa={:.4}{}",
                input_idx, h as u32, alpha_f, img_label
            ),
            (None, None) => format!(
                "[{}:v]setpts=PTS-STARTPTS,format=rgba,colorchannelmixer=aa={:.4}{}",
                input_idx, alpha_f, img_label
            ),
        };
        filter_parts.push(scale_filter);

        let img_overlay = format!(
            "{}{}overlay={}:{}:format=auto{}",
            current_base, img_label,
            ov.x as i32, ov.y as i32,
            next_label
        );
        filter_parts.push(img_overlay);
        current_base = next_label;
    }

    // ── Chat stream overlay ───────────────────────────────────────────────────
    // `current_base` is now the fully-layered base video (with any shapes/images
    // already composited). The chat stream (input 1) goes on top.
    //
    // IMPORTANT: always set shortest=1 on the final overlay.
    //
    // Without it, eof_action=repeat means FFmpeg encodes until the LONGER
    // input ends — if the chat stream covers more time than the base video
    // (e.g. rendering a 4-min clip from a 3-hour stream log), FFmpeg runs
    // for the full chat duration, tripling or worse the encode time and
    // producing a video far longer than the base clip.  shortest=1 makes
    // FFmpeg stop as soon as the base video (input 0) ends.
    let filter_string = if is_luma {
        match (args.overlay_width, args.overlay_height) {
            (Some(ow), Some(oh)) => format!(
                "[1:v]split=2[c][a]; \
                 [c]crop=w=iw/2:h=ih:x=0:y=0[color]; \
                 [a]crop=w=iw/2:h=ih:x=iw/2:y=0,format=gray[alpha]; \
                 [color][alpha]alphamerge[matte]; \
                 [matte]scale={}:{}[scaled_chat]; \
                 {}[scaled_chat]overlay={}:{}:shortest=1:{}[outv]",
                ow, oh, current_base, ox, oy, eof_action
            ),
            _ => format!(
                "[1:v]split=2[c][a]; \
                 [c]crop=w=iw/2:h=ih:x=0:y=0[color]; \
                 [a]crop=w=iw/2:h=ih:x=iw/2:y=0,format=gray[alpha]; \
                 [color][alpha]alphamerge[overlay_v]; \
                 {}[overlay_v]overlay={}:{}:shortest=1:{}[outv]",
                current_base, ox, oy, eof_action
            ),
        }
    } else {
        match (args.overlay_width, args.overlay_height) {
            (Some(ow), Some(oh)) => format!(
                "[1:v]scale={}:{}[scaled_chat]; \
                 {}[scaled_chat]overlay={}:{}:shortest=1:alpha=premultiplied:{}[outv]",
                ow, oh, current_base, ox, oy, eof_action
            ),
            _ => format!(
                "{}[1:v]overlay={}:{}:shortest=1:alpha=premultiplied:{}[outv]",
                current_base, ox, oy, eof_action
            ),
        }
    };

    filter_parts.push(filter_string);

    // Join all filter segments with semicolons.
    // FFmpeg filter_complex uses `;` between filter chains.
    let full_filter = filter_parts.join("; ");

    a.extend([
        "-filter_complex".into(),
        full_filter,
        "-map".into(),
        "[outv]".into(),
        "-map".into(),
        "0:a?".into(),
        "-c:a".into(),
        "copy".into(),
    ]);

    if has_nvenc {
        a.extend([
            "-c:v".into(),
            "h264_nvenc".into(),
            "-preset".into(),
            ffmpeg_preset.into(),
            "-cq".into(),
            "20".into(),
        ]);
    } else {
        a.extend([
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            ffmpeg_preset.into(),
            "-crf".into(),
            "20".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
        ]);
    }

    a.push(args.output_path.clone());
    a
}

/// Build FFmpeg arguments for standalone output (no base video).
///
/// # Standalone pipeline
///
/// ```text
/// [raw BGRA frames] → stdin → FFmpeg encode → output file
/// ```
///
/// Output format depends on `background_mode`:
///
/// * `Transparent` → ProRes 4444 (`.mov`, `yuva444p10le`). No NVENC.
///   ProRes 4444 carries a full 10-bit alpha channel. The software encoder
///   (`prores_ks`) is fast enough at typical chat-overlay resolutions
///   (400×800 @ 24 fps) that hardware encode would not help.
///
/// * Everything else → H.264 (`.mp4`, `yuv420p`). NVENC if available,
///   otherwise `libx264`. Alpha is baked into the frame by the render pass
///   (green / luma / opaque background) so the codec doesn't need to carry it.
fn build_standalone_ffmpeg_args(
    args: &RenderVideoArgs,
    actual_width: i32,
    has_nvenc: bool,
    ffmpeg_preset: &str,
) -> Vec<String> {
    let mut a = vec!["-y".to_string()];

    let (vcodec, pix_fmt) = match args.background_mode {
        // ProRes 4444: true alpha. Software-only — GPU gives no benefit here.
        BackgroundMode::Transparent => ("prores_ks", "yuva444p10le"),
        _ => {
            if has_nvenc {
                ("h264_nvenc", "yuv420p")
            } else {
                ("libx264", "yuv420p")
            }
        }
    };

    a.extend([
        "-thread_queue_size".into(),
        "1024".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "bgra".into(),
        "-s".into(),
        format!("{}x{}", actual_width, args.height),
        "-r".into(),
        args.fps.to_string(),
        "-i".into(),
        "-".into(),
        "-c:v".into(),
        vcodec.into(),
        "-pix_fmt".into(),
        pix_fmt.into(),
    ]);

    if vcodec == "prores_ks" {
        a.extend(["-profile:v".into(), "4444".into()]);
    } else {
        a.extend([
            "-preset".into(),
            ffmpeg_preset.into(),
            // Both flags: NVENC uses -cq, libx264 uses -crf; FFmpeg ignores the other.
            "-cq".into(),
            "20".into(),
            "-crf".into(),
            "20".into(),
        ]);
    }

    a.push(args.output_path.clone());
    a
}

// ──────────────────────────────────────────────────────────────────────────────
// Main render entry point
// ──────────────────────────────────────────────────────────────────────────────
    /**
    /// Seconds elapsed since the start of the requested download range.
    pub range_offset_secs: i64,
    /// Human-readable offset `HH:MM:SS` from the start of the requested download range.
    pub range_offset_str: String,
*/

pub async fn process_chat_render(
    app: &AppHandle,
    tasks: Arc<Mutex<HashMap<String, AppTask>>>,
    task_id: &str,
    input_path: PathBuf,
    mut args: RenderVideoArgs,
    cache_dir_base: PathBuf,
    emote_map: EmoteNameMap,
    cancel_flag: Arc<AtomicBool>,
) -> AppResult<()> {
    // ── Progress helper ───────────────────────────────────────────────────────
    let emit_progress = |progress: f32, text: &str| {
        let mut locked = tasks.lock().unwrap();
        if let Some(task) = locked.get_mut(task_id) {
            task.progress = progress;
            task.status_text = Some(text.to_string());
            let _ = app.emit("task-progress", task.clone());
        }
    };

    emit_progress(1.0, "Scanning chat log & preparing pipeline...");

    clear_token_cache();

    // Immediate-pipe mode always renders transparent (alpha data travels in
    // the pixel stream itself; the background must be clear).
    if args.use_immediate_pipe_overlay {
        args.background_mode = BackgroundMode::Transparent;
    }

    // Fire both probes on background threads immediately so they run
    // concurrently with each other (and later, with the layout pipeline).
    // Each is a subprocess call ~50–150 ms; running in parallel costs only
    // max(t_nvenc, t_probe) instead of the sum.
    let nvenc_probe = std::thread::spawn(probe_nvenc);
    let video_path_for_probe = args.overlay_video_path.clone();
    let fps_for_probe = args.fps;
    let video_frames_probe = std::thread::spawn(move || {
        video_path_for_probe
            .as_deref()
            .and_then(|p| probe_video_frames(p, fps_for_probe))
    });

    // nvenc result is needed right now for chunk-size / preset selection.
    // The join here is essentially free — the thread started microseconds ago
    // and the probe itself takes <150 ms, well within the emote-fetch window.
    let has_nvenc = nvenc_probe.join().unwrap_or(false);

    let is_luma = matches!(args.background_mode, BackgroundMode::LumaMatte);
    // LumaMatte: canvas is doubled horizontally (colour left | mask right).
    let actual_width = if is_luma { args.width * 2 } else { args.width };

    // ── Thread / chunk sizing ─────────────────────────────────────────────────
    //
    // CPU budget:
    //   16 GB RAM, ~4–8 logical CPU cores typical for this class of machine.
    //   Each rayon worker holds one Skia raster surface in thread-local storage.
    //   At 400×800×4 bytes = 1.28 MB per surface (or 2.56 MB for luma-matte
    //   double-width). With 6 workers that's ~15 MB — negligible.
    //
    //   The render loop is CPU-bound (Skia rasterisation). Keeping workers at
    //   n_cores - 1 leaves one core free for the IO writer and the async runtime.
    //
    // Pipe mode uses fewer workers and smaller chunks to minimise frame latency
    // so the consuming FFmpeg process never stalls waiting for input.
    let max_threads = args.max_render_threads;
    let (worker_threads, chunk_size, ffmpeg_preset) = if args.use_immediate_pipe_overlay {
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .min(max_threads.unwrap_or(4));
        let preset = if has_nvenc { "p1" } else { "ultrafast" };
        (n, CHUNK_SIZE_BASE, preset)
    } else {
        match args.quality_preset {
            QualityPreset::Draft => (
                max_threads.unwrap_or(1).max(1),
                CHUNK_SIZE_BASE,
                if has_nvenc { "p1" } else { "ultrafast" },
            ),
            QualityPreset::Standard => {
                let cpus = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4);
                // Leave 1 core for IO + async. Cap at 6 to avoid surface RAM explosion.
                let n = max_threads.unwrap_or_else(|| cpus.saturating_sub(1).clamp(1, 6));
                (
                    n,
                    CHUNK_SIZE_BASE * n,
                    if has_nvenc { "p3" } else { "veryfast" },
                )
            }
            QualityPreset::High => {
                let cpus = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4);
                let n = max_threads.unwrap_or_else(|| cpus.saturating_sub(1).clamp(1, 8));
                (
                    n,
                    CHUNK_SIZE_BASE * n,
                    if has_nvenc { "p5" } else { "fast" },
                )
            }
        }
    };

    let render_pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(worker_threads)
            .thread_name(|i| format!("engine-worker-{}", i))
            .build()
            .map_err(|e| AppError::InternalError(format!("Failed to build render pool: {}", e)))?,
    );

    // ── FFmpeg argument construction ──────────────────────────────────────────
    let ffmpeg_args = if args.overlay_video_path.is_some() {
        build_overlay_ffmpeg_args(&args, actual_width, is_luma, has_nvenc, ffmpeg_preset)
    } else {
        build_standalone_ffmpeg_args(&args, actual_width, has_nvenc, ffmpeg_preset)
    };

    // ── Spawn FFmpeg ──────────────────────────────────────────────────────────
    let mut ffmpeg_child = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdin(Stdio::piped())
        // Capture stderr so FFmpeg errors surface instead of being silently
        // swallowed. We read it after the process exits via wait_with_output,
        // or drain it on a thread so it doesn't fill the OS pipe buffer and
        // deadlock the process. Here we inherit (null would hide errors; piped
        // without draining deadlocks on long encodes with verbose FFmpeg output).
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;

    let ff_stdin = ffmpeg_child.stdin.take().unwrap();

    // Bounded channel: backpressure from FFmpeg naturally throttles rendering.
    let (io_tx, io_rx) = crossbeam_channel::bounded::<Arc<ReusableBuffer>>(IO_CHANNEL_DEPTH);

    // IO writer thread: one dedicated thread drains the channel and writes
    // raw pixel data into FFmpeg stdin. The large BufWriter amortises syscall
    // overhead; 8 MiB is a good balance between RAM usage and write batching.
    let io_thread = std::thread::spawn(move || {
        // 8 MiB write buffer — amortises small per-frame writes at high res / fps.
        let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, ff_stdin);
        while let Ok(frame) = io_rx.recv() {
            if let Some(data) = &frame.data {
                if writer.write_all(data).is_err() {
                    break;
                }
            }
        }
        let _ = writer.flush();
        // Drain the channel so senders can unblock and detect the closed pipe.
        drop(writer);
        for _ in io_rx {}
    });

    // ── Scan pass — collect emote IDs, image URLs, timing info ────────────────
    let mut emote_ids: FxHashSet<i32> = FxHashSet::default();
    let mut image_urls: FxHashSet<String> = FxHashSet::default();

    // Pre-seed image URLs from the emote map (provider-filtered).
    for url in emote_map.all_urls_filtered(&args.emote_providers) {
        image_urls.insert(url.to_string());
    }

    let skip_users_set: FxHashSet<String> = args.skip_users.iter().cloned().collect();

    // Channel for raw messages from the scan thread → layout thread.
    let (loader_tx, loader_rx) = crossbeam_channel::bounded::<(MessageSaved, bool)>(4096);
    // Separate channel for messages that arrived before time_zero — these are
    // pre-filled into the overlay at frame 0 as already-settled bubbles.
    // Bounded at 512: prefill candidates are at most message_hold_seconds of
    // chat (typically <100 messages), so this never fills up in practice.
    let (prefill_tx, prefill_rx) =
        crossbeam_channel::bounded::<(MessageSaved, bool, u32)>(512);
    // Channel for baked ScheduledMessages from the layout thread → render loop.
    let (stamp_tx, stamp_rx) = crossbeam_channel::bounded::<(u32, Arc<ScheduledMessage>)>(2048);

    let scan_path = input_path.clone();
    let scan_cancel = Arc::clone(&cancel_flag);
    let scan_skip = skip_users_set.clone();
    let scan_args = args.clone();
    let scan_emote_map = emote_map.clone();
    let group_window = args.group_messages_window_secs as i64;
    let group_enabled = args.group_messages;
    let do_prefill = args.prefill_from_start && args.time_zero_ms.is_some();
    let prefill_window_secs = args.message_hold_seconds as i64;

    // Single-shot channel that carries (max_offset_sec, base_ts, emote_ids, image_urls)
    // from the scan thread back to the async task.
    let (meta_tx, meta_rx) = tokio::sync::oneshot::channel::<(f64, Vec<i32>, Vec<String>)>();

    std::thread::spawn(move || {
        let f = match std::fs::File::open(&scan_path) {
            Ok(f) => f,
            Err(_) => return,
        };
        // 2 MiB read buffer: large enough to process multi-MB log files
        // without thrashing the OS page cache.
        let reader = std::io::BufReader::with_capacity(2 << 20, f);

        let mut max_offset_sec: f64 = 0.0;
        let mut last_user = String::new();
        let mut last_time = -1i64;

        let flags = &scan_args.emote_providers;
        let map_opt = if !scan_emote_map.is_empty() && flags.any_name_provider_enabled() {
            Some((&scan_emote_map, flags))
        } else {
            None
        };

        for line in std::io::BufRead::lines(reader).flatten() {
            if scan_cancel.load(Ordering::Relaxed) {
                break;
            }

            let msg: MessageSaved = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if scan_skip.contains(&msg.sender.username) {
                continue;
            }

            if let Some(start) = scan_args.start_ms {
                if (msg.created_at_secs as u64 * 1000) < start {
                    continue;
                }
            }
            if let Some(end) = scan_args.end_ms {
                if (msg.created_at_secs as u64 * 1000) > end {
                    break;
                }
            }

            let offset = msg.range_offset_secs as f64;
            if offset > max_offset_sec {
                max_offset_sec = offset;
            }

            // Route prefill candidates — messages that arrived before time_zero
            // but within the hold window — to the prefill channel.  They will
            // be injected at frame 0 with their age already baked in so timers
            // still expire at the right moment.
            if do_prefill && offset < 0.0 {
                let age_secs = -offset;
                if age_secs <= prefill_window_secs as f64 {
                    // Compute how many frames old this message already is at frame 0.
                    let age_offset_frames =
                        (age_secs * scan_args.fps as f64).round() as u32;
                    let _ = prefill_tx.send((msg, false /* never grouped for prefill */, age_offset_frames));
                }
                // Collect emote IDs for prefill messages too.
                continue;
            }

            // Collect emote IDs / image URLs from this message's tokens.
            for tok in tokenise(&msg.content, map_opt) {
                match tok {
                    MessageToken::KickEmote { id, .. } => {
                        if flags.kick {
                            if let Ok(i) = id.parse::<i32>() {
                                emote_ids.insert(i);
                            }
                        }
                    }
                    MessageToken::ProviderEmote(e) => {
                        image_urls.insert(e.url.to_string());
                    }
                    MessageToken::ImageUrl(url) => {
                        if flags.image_urls {
                            image_urls.insert(url.to_string());
                        }
                    }
                    _ => {}
                }
            }

            let is_grouped = group_enabled
                && msg.sender.username == last_user
                && (msg.range_offset_secs - last_time) <= group_window;
            last_user.clear();
            last_user.push_str(&msg.sender.username);
            last_time = msg.range_offset_secs;

            if loader_tx.send((msg, is_grouped)).is_err() {
                break;
            }
        }

        let _ = meta_tx.send((
            max_offset_sec,
            emote_ids.into_iter().collect(),
            image_urls.into_iter().collect(),
        ));
    });

    let (max_offset_sec, emote_ids, image_urls) =
        meta_rx.await.unwrap_or((0.0, Vec::new(), Vec::new()));

    emit_progress(5.0, "Hydrating emote caches...");

    // ── Cache warm-up ─────────────────────────────────────────────────────────
    let target_emote_h = ((args.font_size + args.line_spacing as f32) * 1.25).ceil() as u32;

    let emote_cache = Arc::new(EmoteCache::new(
        cache_dir_base.join("emote_cache"),
        args.max_cached_emotes,
        target_emote_h,
        args.quality_preset.clone(),
        args.eager_gif_decode,
    ));
    let img_cache = Arc::new(ImageCache::new(
        cache_dir_base.join("image_cache"),
        args.max_cached_emotes,
        // Image URLs are typically larger than emotes; give them 4× the height
        // budget so they render at a legible size in the chat stream.
        target_emote_h * 4,
        args.quality_preset.clone(),
        args.eager_gif_decode,
    ));

    // Fetch both caches concurrently — network bound, so parallelism helps.
    let (emote_result, img_result) = tokio::join!(
        emote_cache.ensure_cached(&emote_ids),
        img_cache.ensure_cached(&image_urls),
    );
    emote_result?;
    img_result?;

    // NOTE: overlay_images (Skia-loaded image assets) are no longer used.
    // Image overlays are composited by FFmpeg via the filter_complex chain built
    // in build_overlay_ffmpeg_args.

    // ── Font setup ────────────────────────────────────────────────────────────
    // FontMgr (RCHandle<SkFontMgr>) is !Send, so it must NOT be alive across
    // any .await point — the compiler would reject the future as !Send, which
    // prevents tauri::async_runtime::spawn from accepting process_chat_render.
    // Scope font_mgr to a block so it is dropped before the first .await that
    // follows (the spawn_blocking at the end of this function). Only typeface
    // escapes the block; Typeface (RCHandle<SkTypeface>) IS Send.
    let (message_font, username_font, msg_line_h, metrics_ascent) = {
        let font_mgr = FontMgr::new();
        let typeface = font_mgr
            .match_family_style(&args.font_name, FontStyle::normal())
            .or_else(|| font_mgr.match_family_style("Apple Color Emoji", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("Segoe UI Emoji", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("Noto Color Emoji", FontStyle::normal()))
            .ok_or_else(|| AppError::InternalError("No system fonts found".into()))?;
        // font_mgr is dropped here — before any subsequent .await.
        let mf = Font::from_typeface(typeface.clone(), args.font_size);
        let uf = Font::from_typeface(typeface, (args.font_size * 0.95).max(12.0));
        let (_, met) = mf.metrics();
        let lh = (met.descent - met.ascent) + args.line_spacing as f32;
        (mf, uf, lh, met.ascent)
    };

    // ── Layout thread setup ───────────────────────────────────────────────────
    let args_pr = args.clone();
    let emote_cache_pr = Arc::clone(&emote_cache);
    let img_cache_pr = Arc::clone(&img_cache);
    let pr_cancel = Arc::clone(&cancel_flag);
    let render_pool_pr = Arc::clone(&render_pool);
    let highlight_set: FxHashSet<String> = args.pinned_users.iter().cloned().collect();
    let emote_map_pr = emote_map.clone();

    std::thread::spawn(move || {
        // ── Prefill pass ──────────────────────────────────────────────────────
        // Only active when prefill_from_start=true AND time_zero_ms is set.
        // When disabled, prefill_rx is empty (scan thread never sends to it)
        // and prefill_tx is dropped at scan-thread-end, so iter() terminates
        // instantly — but we skip the whole block for clarity and zero overhead.
        if do_prefill {
            let mut prefill_msgs: Vec<(MessageSaved, bool, u32)> =
                prefill_rx.iter().collect();
            // Sort oldest-first (most negative offset = arrived earliest).
            // age_offset_frames is larger for older messages.
            prefill_msgs.sort_unstable_by(|a, b| b.2.cmp(&a.2));

            if !prefill_msgs.is_empty() {
                let (pfx, pfrx) = crossbeam_channel::bounded::<
                    Vec<Option<ScheduledMessage>>,
                >(1);
                let args_c = args_pr.clone();
                let ec = emote_cache_pr.clone();
                let ic = img_cache_pr.clone();
                let hl = highlight_set.clone();
                let em = emote_map_pr.clone();
                let uf = username_font.clone();
                let mf = message_font.clone();
                let mh = msg_line_h;
                let ma = metrics_ascent;

                render_pool_pr.spawn(move || {
                    let results: Vec<Option<ScheduledMessage>> = prefill_msgs
                        .into_par_iter()
                        .map(|(msg, _is_grouped, age_offset)| {
                            PRE_RENDER_MEASURE_CACHE.with(|cc| {
                                MEASURE_GENERATION.with(|gc| {
                                    let mut mc = cc.borrow_mut();
                                    let mut tg = gc.borrow_mut();
                                    *tg = 0;
                                    let is_highlighted =
                                        hl.contains(&msg.sender.username);
                                    match layout_message_blocking(
                                        &msg.content,
                                        &msg.sender.username,
                                        &msg.sender.identity.color,
                                        &uf,
                                        &mf,
                                        (args_c.width - 2 * args_c.padding) as f32,
                                        mh,
                                        ma,
                                        &ec,
                                        &ic,
                                        &args_c,
                                        &em,
                                        &mut mc,
                                        0,
                                        false, // never grouped at prefill
                                    ) {
                                        Ok((lines, bw, bh, uc)) => {
                                            if lines.is_empty()
                                                || lines.iter().all(|l| l.tokens.is_empty())
                                            {
                                                return None;
                                            }
                                            Some(ScheduledMessage::new_prefill(
                                                age_offset,
                                                lines,
                                                bw,
                                                bh,
                                                Color::from(&args_c.bubble_color),
                                                uc,
                                                false,
                                                is_highlighted,
                                            ))
                                        }
                                        Err(_) => None,
                                    }
                                })
                            })
                        })
                        .collect();
                    let _ = pfx.send(results);
                });

                // Drain results in order — prefill messages are already sorted
                // oldest-first, so stamp_tx receives them in the right order.
                if let Ok(results) = pfrx.recv() {
                    for sched in results.into_iter().flatten() {
                        let msg = Arc::new(sched);
                        let _ = stamp_tx.send((0, msg));
                    }
                }
            }
        }

        // ── Sliding-window layout pipeline ────────────────────────────────────
        //
        // Old design (stop-the-world):
        //   recv 128 msgs → block rayon until all 128 done → send results → repeat
        //   Problem: rayon is idle during recv; recv is blocked during rayon.
        //
        // New design (sliding window):
        //   Keep up to MAX_BATCHES_IN_FLIGHT batches submitted to rayon at once.
        //   Each batch is a rayon `scope` task that sends its results back via a
        //   per-batch channel. The layout thread round-robins across in-flight
        //   batches, draining completed results in submission order so that
        //   spawn_frame assignment remains strictly monotonic.
        //
        //   This keeps both the recv path and rayon workers busy simultaneously,
        //   eliminating the staircase pipeline stall between batches. The benefit
        //   is most visible for very long VODs (millions of messages) where the
        //   old design caused the render loop to starve waiting for the layout
        //   thread to unblock.
        //
        //   MAX_BATCHES_IN_FLIGHT = 4 is a good default: with 128-msg batches
        //   that is 512 messages queued to rayon at once, which is ~2–4× the
        //   worker count, keeping all cores busy without unbounded RAM growth.

        const BATCH_SIZE: usize = 128;
        const MAX_BATCHES_IN_FLIGHT: usize = 4;

        type BatchResult = Vec<Option<(i64, ScheduledMessage)>>;
        // Each in-flight batch sends its results back through a oneshot-style
        // crossbeam channel. Indexed FIFO so drain order matches submission order.
        let mut in_flight: VecDeque<crossbeam_channel::Receiver<BatchResult>> =
            VecDeque::with_capacity(MAX_BATCHES_IN_FLIGHT);

        let mut batch: Vec<(MessageSaved, bool)> = Vec::with_capacity(BATCH_SIZE);
        let mut last_assigned_frame = -1i64;
        let mut layout_gen: u32 = 0;

        let submit_batch = |msgs: Vec<(MessageSaved, bool)>,
                            gen: u32,
                            pool: &rayon::ThreadPool|
            -> crossbeam_channel::Receiver<BatchResult> {
            let (tx, rx) = crossbeam_channel::bounded::<BatchResult>(1);
            let args_c = args_pr.clone();
            let ec = emote_cache_pr.clone();
            let ic = img_cache_pr.clone();
            let hl = highlight_set.clone();
            let em = emote_map_pr.clone();
            let uf = username_font.clone();
            let mf = message_font.clone();
            let mh = msg_line_h;
            let ma = metrics_ascent;

            pool.spawn(move || {
                let results: BatchResult = msgs
                    .into_par_iter()
                    .map(|(msg, is_grouped)| {
                        PRE_RENDER_MEASURE_CACHE.with(|cc| {
                            MEASURE_GENERATION.with(|gc| {
                                let mut mc = cc.borrow_mut();
                                let mut tg = gc.borrow_mut();
                                *tg = gen;

                                let offset_sec = (msg.range_offset_secs as f64).max(0.0);
                                let base_frame =
                                    (offset_sec * args_c.fps as f64).round() as i64;
                                let is_highlighted = hl.contains(&msg.sender.username);

                                match layout_message_blocking(
                                    &msg.content,
                                    &msg.sender.username,
                                    &msg.sender.identity.color,
                                    &uf,
                                    &mf,
                                    (args_c.width - 2 * args_c.padding) as f32,
                                    mh,
                                    ma,
                                    &ec,
                                    &ic,
                                    &args_c,
                                    &em,
                                    &mut mc,
                                    gen,
                                    is_grouped,
                                ) {
                                    Ok((lines, bubble_w, bubble_h, user_color)) => {
                                        // Drop messages whose content was entirely
                                        // filtered (e.g. emote-only messages when
                                        // the kick provider flag is off). An empty
                                        // layout still produces a valid bubble_h
                                        // (just padding) that displaces other
                                        // messages as a phantom black rectangle.
                                        if lines.is_empty() || lines.iter().all(|l| l.tokens.is_empty()) {
                                            return None;
                                        }
                                        Some((
                                            base_frame,
                                            ScheduledMessage::new(
                                                0,
                                                lines,
                                                bubble_w,
                                                bubble_h,
                                                Color::from(&args_c.bubble_color),
                                                user_color,
                                                is_grouped,
                                                is_highlighted,
                                            ),
                                        ))
                                    }
                                    Err(_) => None,
                                }
                            })
                        })
                    })
                    .collect();

                let _ = tx.send(results);
            });

            rx
        };

        let drain_one = |rx: crossbeam_channel::Receiver<BatchResult>,
                         last_frame: &mut i64,
                         stamp: &crossbeam_channel::Sender<(u32, Arc<ScheduledMessage>)>|
            -> bool {
            let results = match rx.recv() {
                Ok(r) => r,
                Err(_) => return false,
            };
            let mut cursor = *last_frame;
            for (base_frame, mut sched) in results.into_iter().flatten() {
                let assigned = if base_frame <= cursor {
                    cursor + 2
                } else {
                    base_frame
                };
                cursor = assigned;
                sched.spawn_frame = assigned as u32;
                if stamp.send((sched.spawn_frame, Arc::new(sched))).is_err() {
                    return false;
                }
            }
            *last_frame = cursor;
            true
        };

        // ── Main recv loop ────────────────────────────────────────────────────
        loop {
            if pr_cancel.load(Ordering::Relaxed) {
                break;
            }

            match loader_rx.try_recv() {
                Ok(msg_tuple) => {
                    batch.push(msg_tuple);
                    if batch.len() >= BATCH_SIZE {
                        // If the in-flight queue is full, drain the oldest batch
                        // before submitting a new one — provides back-pressure so
                        // we don't queue unlimited work ahead of the render loop.
                        if in_flight.len() >= MAX_BATCHES_IN_FLIGHT {
                            if let Some(rx) = in_flight.pop_front() {
                                if !drain_one(rx, &mut last_assigned_frame, &stamp_tx) {
                                    break;
                                }
                            }
                        }
                        layout_gen = layout_gen.wrapping_add(1);
                        let rx = submit_batch(
                            std::mem::take(&mut batch),
                            layout_gen,
                            &render_pool_pr,
                        );
                        in_flight.push_back(rx);
                        batch.reserve(BATCH_SIZE);
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    // No new messages yet — drain the oldest in-flight batch
                    // (if any) rather than busy-spinning.
                    if let Some(rx) = in_flight.pop_front() {
                        if !drain_one(rx, &mut last_assigned_frame, &stamp_tx) {
                            break;
                        }
                    } else {
                        // Both the channel and in-flight queue are empty.
                        // Block briefly so we don't burn a core on hot polling.
                        match loader_rx.recv_timeout(std::time::Duration::from_millis(5)) {
                            Ok(msg_tuple) => batch.push(msg_tuple),
                            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        }
                    }
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }

        // ── Flush final partial batch ─────────────────────────────────────────
        if !batch.is_empty() {
            layout_gen = layout_gen.wrapping_add(1);
            let rx = submit_batch(batch, layout_gen, &render_pool_pr);
            in_flight.push_back(rx);
        }

        // Drain any remaining in-flight batches in submission order.
        for rx in in_flight {
            if !drain_one(rx, &mut last_assigned_frame, &stamp_tx) {
                break;
            }
        }
    });

    // ── Frame render loop ─────────────────────────────────────────────────────
    //
    // total_frames is derived from the chat log's time span plus hold time.
    // In overlay mode the chat log may cover a much wider window than the
    // actual base video clip (e.g. a 4-min clip from a 3-hr stream VOD log).
    // Rendering more frames than the video has is pure waste: the extra frames
    // travel through stdin, get decoded by FFmpeg, and are discarded because
    // shortest=1 stops the output at the video end.
    //
    // Probe the base video duration with ffprobe so we can cap total_frames.
    // ffprobe is a read-only metadata query — typically <100 ms even on large
    // files because it only reads the container header.
    let chat_total_frames = ((max_offset_sec * args.fps as f64).round() as u32)
        .saturating_add(args.message_hold_seconds * args.fps);

    // The probe thread was started long before the layout pipeline began.
    // By the time we reach here (after scan + layout + emote fetch), the
    // ffprobe subprocess has been done for several seconds.  This join is free.
    let total_frames = {
        let probed = video_frames_probe.join().unwrap_or(None);
        probed
            .unwrap_or(chat_total_frames)
            .min(chat_total_frames)
    };

    let bg_color = match args.background_mode {
        BackgroundMode::Transparent => Color::TRANSPARENT,
        BackgroundMode::LumaMatte => Color::BLACK,
        BackgroundMode::ChromaKeyGreen => Color::from_argb(255, 0, 255, 0),
        BackgroundMode::CustomColor => Color::from(&args.background_color),
    };

    // BGRA8888 + Premul: matches FFmpeg input format exactly, zero pixel
    // conversion overhead on the CPU side. All rendering is CPU-only.
    let info = ImageInfo::new(
        (actual_width, args.height),
        ColorType::BGRA8888,
        AlphaType::Premul,
        None,
    );
    let num_bytes = (actual_width * args.height * 4) as usize;

    let pixel_pool = Arc::new(PixelBufferPool::new(
        IO_CHANNEL_DEPTH + POOL_HEADROOM + chunk_size,
    ));

    let mut active_bubbles: VecDeque<Arc<ScheduledMessage>> = VecDeque::new();
    let mut next_stamp: Option<(u32, Arc<ScheduledMessage>)> = None;
    // Pre-allocate the chunk vector to avoid per-chunk heap growth.
    // None = repeat last frame (deduped); Some = dirty frame needing a render.
    let mut frame_chunk: Vec<(u32, Option<Vec<Arc<ScheduledMessage>>>)> = Vec::with_capacity(chunk_size);

    let fps_f32 = args.fps as f32;
    let hold_secs = args.message_hold_seconds as f32;
    let fade_secs = args.message_fade_out_seconds as f32;
    let anim_slide = args.anim_slide;
    let anim_fade = args.anim_fade_in;
    let eviction = args.eviction_strategy.clone();
    let canvas_max_h = args.height - args.padding;

    let mut last_sig: u64 = u64::MAX;
    let mut prev_animating: bool = false;
    let mut last_buf: Option<Arc<ReusableBuffer>> = None;

    emit_progress(10.0, "Rendering frames...");

    'frame: for f_idx in 0..total_frames {
        if cancel_flag.load(Ordering::Relaxed) {
            break;
        }

        // Drain any newly spawned bubbles whose frame has arrived.
        loop {
            if next_stamp.is_none() {
                next_stamp = stamp_rx.try_recv().ok();
            }
            match &next_stamp {
                Some((spawn_frame, _)) if *spawn_frame <= f_idx => {
                    active_bubbles.push_front(next_stamp.take().unwrap().1);
                }
                _ => break,
            }
        }

        // Evict timed-out bubbles (Timed strategy only).
        if matches!(eviction, EvictionStrategy::Timed) {
            let max_age = hold_secs + fade_secs;
            active_bubbles
                .retain(|b| f_idx.saturating_sub(b.spawn_frame) as f32 / fps_f32 <= max_age);
        }

        // Trim bubbles that scroll off the top of the canvas.
        // Keep one extra bubble beyond the visible cutoff so the scroll
        // transition looks smooth rather than popping.
        let mut acc_h = 0i32;
        let mut keep = active_bubbles.len();
        for (i, b) in active_bubbles.iter().enumerate() {
            acc_h += b.bubble_h + args.message_spacing;
            if acc_h >= canvas_max_h {
                keep = i + 2;
                break;
            }
        }
        if keep < active_bubbles.len() {
            active_bubbles.truncate(keep);
        }

        // ── Per-frame dedup check ─────────────────────────────────────────────
        // Compute the frame signature directly on &active_bubbles so we never
        // clone the bubble list unless the frame is genuinely dirty.  At 30 fps
        // with a message arriving once per second only ~1 in 30 frames is dirty;
        // the old code cloned into a Vec<Arc<...>> for every frame regardless.
        {
            let (sig, is_animating) = if active_bubbles.is_empty() {
                (0u64, false)
            } else {
                let s = frame_signature_deque(
                    &active_bubbles,
                    f_idx,
                    fps_f32,
                    anim_slide,
                    anim_fade,
                    &eviction,
                    hold_secs,
                    fade_secs,
                );
                let anim = active_bubbles.iter().any(|b| {
                    b.is_animating(f_idx, fps_f32, anim_slide, anim_fade, &eviction, hold_secs)
                });
                (s, anim)
            };

            let dirty = sig != last_sig || prev_animating || last_buf.is_none();
            if dirty {
                // Only now pay for the clone — and only for the bubbles visible
                // on this frame (already trimmed above).
                let vis: Vec<Arc<ScheduledMessage>> = active_bubbles.iter().cloned().collect();
                frame_chunk.push((f_idx, Some(vis)));
                last_sig = sig;
            } else {
                frame_chunk.push((f_idx, None)); // reuse last_buf
            }
            prev_animating = is_animating;
        }

        if frame_chunk.len() < chunk_size && f_idx < total_frames - 1 {
            continue;
        }

        // ── Build unique job list from chunk ──────────────────────────────────
        // `sequence` maps each frame to Ok(job_index) (fresh render) or Err(())
        // (repeat last_buf). None-vis frames are already known to be repeats.
        let mut unique_jobs: Vec<(u32, Vec<Arc<ScheduledMessage>>)> = Vec::new();
        let mut sequence: Vec<Result<usize, ()>> = Vec::with_capacity(frame_chunk.len());

        for (frame_id, vis_opt) in &mut frame_chunk {
            match vis_opt.take() {
                Some(bubbles) => {
                    let job_idx = unique_jobs.len();
                    unique_jobs.push((*frame_id, bubbles));
                    sequence.push(Ok(job_idx));
                }
                None => {
                    sequence.push(Err(()));
                }
            }
        }

        // ── Parallel render ───────────────────────────────────────────────────
        if !unique_jobs.is_empty() {
            let pool = Arc::clone(&pixel_pool);
            let args_block = args.clone();
            let info_clone = info.clone();

            let rendered_jobs: Vec<Arc<ReusableBuffer>> = render_pool.install(|| {
                unique_jobs
                    .into_par_iter()
                    .map(|(frame_id, bubbles)| {
                        let mut buf = ReusableBuffer::new(pool.clone(), num_bytes);

                        SKIA_SURFACE.with(|surf_cell| {
                            let mut surf_opt = surf_cell.borrow_mut();

                            // Lazily initialise or reallocate the per-thread surface.
                            // In practice this branch is taken exactly once per thread.
                            if surf_opt.is_none()
                                || surf_opt.as_ref().unwrap().width() != actual_width
                                || surf_opt.as_ref().unwrap().height() != args_block.height
                            {
                                // Create the surface with the same ImageInfo used for
                                // read_pixels so the pixel format matches exactly on every
                                // platform. raster_n32_premul uses the host-native byte order
                                // (kN32) which is BGRA on little-endian x86 but diverges on
                                // other targets; using surfaces::raster() with an explicit
                                // BGRA8888 info guarantees a consistent layout that FFmpeg
                                // can consume without channel-swapping.
                                *surf_opt = Some(
                                    surfaces::raster(&info_clone, None, None)
                                        .unwrap(),
                                );
                            }

                            let surface = surf_opt.as_mut().unwrap();
                            let canvas = surface.canvas();

                            draw_frame(
                                canvas,
                                &bubbles,
                                &args_block,
                                bg_color,
                                is_luma,
                                frame_id,
                                fps_f32,
                                hold_secs,
                                anim_slide,
                                anim_fade,
                                &eviction,
                            );

                            // Read pixels out into the pool buffer. The stride must
                            // match `actual_width * 4` bytes exactly.
                            surface.read_pixels(
                                &info_clone,
                                buf.data.as_mut().unwrap().as_mut_slice(),
                                (actual_width * 4) as usize,
                                (0, 0),
                            );
                        });

                        Arc::new(buf)
                    })
                    .collect()
            });

            // ── Dispatch to IO thread ─────────────────────────────────────────
            let mut channel_closed = false;
            let mut render_iter = rendered_jobs.into_iter();

            for directive in &sequence {
                let buf_to_send = match directive {
                    Ok(_) => {
                        let buf = render_iter.next().unwrap();
                        last_buf = Some(Arc::clone(&buf));
                        buf
                    }
                    // Repeat: clone the Arc (cheap) so the IO thread sees
                    // a reference to the exact same pixel data.
                    Err(()) => Arc::clone(last_buf.as_ref().unwrap()),
                };

                if io_tx.send(buf_to_send).is_err() {
                    channel_closed = true;
                    break;
                }
            }

            if channel_closed {
                break 'frame;
            }
        } else if let Some(ref buf) = last_buf {
            // Entire chunk was identical — blast the same buffer pointer N times.
            let buf = Arc::clone(buf);
            for _ in &sequence {
                if io_tx.send(Arc::clone(&buf)).is_err() {
                    break 'frame;
                }
            }
        }

        frame_chunk.clear();

        let pct = 10.0 + ((f_idx as f32 / total_frames as f32) * 90.0);
        emit_progress(pct, &format!("Rendering... ({:.1}%)", pct));
    }

    // Signal the IO thread that no more frames are coming, then close stdin
    // so FFmpeg sees EOF on the rawvideo stream and can finish encoding.
    drop(io_tx);

    // io_thread.join() and ffmpeg_child.wait() are both blocking syscalls.
    // Calling them directly on a Tokio async task thread would block the
    // entire executor — the UI freezes at 100%, Tauri events stop, and
    // cancellation becomes unresponsive for the full duration of FFmpeg's
    // final mux/flush (can be 10s+ on long videos). Offload to a dedicated
    // OS thread via spawn_blocking so the async runtime stays live.
    let cancelled = cancel_flag.load(Ordering::SeqCst);

    let shutdown_result = tokio::task::spawn_blocking(move || {
        if cancelled {
            // Kill FFmpeg immediately; IO thread will see broken pipe and exit.
            let _ = ffmpeg_child.kill();
        }
        // Wait for the IO thread to finish flushing its 8 MiB BufWriter and
        // closing stdin. This must come before wait() so FFmpeg sees EOF.
        let _ = io_thread.join();
        // Now wait for FFmpeg to finish encoding. For long videos this can
        // take several seconds even after stdin closes (final mux pass).
        ffmpeg_child.wait()
    })
        .await;

    if cancelled {
        emit_progress(100.0, "Render Cancelled");
        return Err(AppError::InternalError("Cancelled by user".into()));
    }

    match shutdown_result {
        Ok(Ok(status)) if status.success() => {
            emit_progress(100.0, "Complete");
            Ok(())
        }
        Ok(Ok(status)) => {
            emit_progress(100.0, "Encoding failed");
            Err(AppError::Ffmpeg(format!(
                "FFmpeg exited with status {}",
                status
            )))
        }
        Ok(Err(e)) => {
            emit_progress(100.0, "Encoding failed");
            Err(AppError::Ffmpeg(format!("FFmpeg wait error: {}", e)))
        }
        Err(e) => {
            emit_progress(100.0, "Encoding failed");
            Err(AppError::InternalError(format!(
                "spawn_blocking panicked: {}",
                e
            )))
        }
    }
}