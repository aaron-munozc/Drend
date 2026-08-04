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
    BackgroundMode, CustomImageOverlay, EvictionStrategy, QualityPreset, RenderVideoArgs,
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
    static PAINT_SHAPE_OVERLAY: RefCell<Paint> = RefCell::new({
        let mut p = Paint::default(); p.set_anti_alias(true); p
    });
    static PAINT_IMAGE_OVERLAY: RefCell<Paint> = RefCell::new(Paint::default());
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
            if buf.capacity() < min_len {
                buf.reserve(min_len - buf.len());
            }
            // SAFETY: caller is about to fill the buffer; content is don't-care.
            unsafe { buf.set_len(min_len) };
            return buf;
        }
        drop(guard);
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
        // Walk tokens once to detect animated emotes and their period.
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

    /// Returns `true` when this bubble is in any animated state (entrance anim,
    /// fade-out, or contains GIF emotes). Used to decide whether a frame can be
    /// reused from the previous render.
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
        let age = (frame_id.saturating_sub(self.spawn_frame)) as f32 / fps;
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

        // Binary search for the largest prefix that fits.
        let char_count = remainder.chars().count();
        let mut lo = 1usize;
        let mut hi = char_count.saturating_sub(1).max(1);
        let mut best_byte = 0usize;

        while lo <= hi {
            let mid = (lo + hi) / 2;
            let byte_off = remainder
                .char_indices()
                .nth(mid)
                .map(|(i, _)| i)
                .unwrap_or(remainder.len());

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
            // Even a single character doesn't fit — emit it anyway to avoid
            // an infinite loop on extremely narrow canvases.
            let end = remainder
                .char_indices()
                .nth(1)
                .map(|(i, _)| i)
                .unwrap_or(remainder.len());
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
                    }
                }
                MessageToken::ProviderEmote(ResolvedEmote {
                    url, zero_width, ..
                }) => {
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
                    }
                }
                MessageToken::ImageUrl(url) => {
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
// Image overlay cache
// ──────────────────────────────────────────────────────────────────────────────

struct OverlayImageEntry {
    img: skia_safe::Image,
    native_w: f32,
    native_h: f32,
}

fn load_overlay_images(overlays: &[CustomImageOverlay]) -> FxHashMap<String, OverlayImageEntry> {
    let mut map = FxHashMap::default();
    for ov in overlays {
        if map.contains_key(&ov.asset_path) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&ov.asset_path) {
            use std::io::Cursor;
            if let Ok(dyn_img) = image::ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .and_then(|r| {
                    r.decode()
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                })
            {
                let rgba = dyn_img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let data = skia_safe::Data::new_copy(rgba.as_raw());
                let info = ImageInfo::new(
                    (w as i32, h as i32),
                    ColorType::RGBA8888,
                    AlphaType::Premul,
                    None,
                );
                if let Some(img) =
                    skia_safe::images::raster_from_data(&info, &data, (w * 4) as usize)
                {
                    map.insert(
                        ov.asset_path.clone(),
                        OverlayImageEntry {
                            img,
                            native_w: w as f32,
                            native_h: h as f32,
                        },
                    );
                }
            }
        }
    }
    map
}

// ──────────────────────────────────────────────────────────────────────────────
// Draw mid-layer overlays (shapes + images)
// ──────────────────────────────────────────────────────────────────────────────

#[inline]
fn draw_mid_layer(
    canvas: &skia_safe::Canvas,
    args: &RenderVideoArgs,
    overlay_images: &FxHashMap<String, OverlayImageEntry>,
) {
    if !args.shape_overlays.is_empty() {
        PAINT_SHAPE_OVERLAY.with(|cell| {
            let mut p = cell.borrow_mut();
            for shape in &args.shape_overlays {
                p.set_color(Color::from(&shape.color));
                let rect = Rect::new(
                    shape.x,
                    shape.y,
                    shape.x + shape.width,
                    shape.y + shape.height,
                );
                canvas.draw_round_rect(rect, shape.corner_radius, shape.corner_radius, &p);
            }
        });
    }

    if !args.image_overlays.is_empty() {
        PAINT_IMAGE_OVERLAY.with(|cell| {
            let mut p = cell.borrow_mut();
            for ov in &args.image_overlays {
                if let Some(entry) = overlay_images.get(&ov.asset_path) {
                    let draw_w = ov.width.unwrap_or(entry.native_w);
                    let draw_h = ov.height.unwrap_or(entry.native_h);
                    let dest = Rect::new(ov.x, ov.y, ov.x + draw_w, ov.y + draw_h);
                    p.set_alpha_f(ov.alpha.clamp(0.0, 1.0));
                    canvas.draw_image_rect(&entry.img, None, dest, &p);
                }
            }
        });
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Core frame draw
// ──────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_frame(
    canvas: &skia_safe::Canvas,
    bubbles: &[Arc<ScheduledMessage>],
    args: &RenderVideoArgs,
    overlay_images: &FxHashMap<String, OverlayImageEntry>,
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
    draw_mid_layer(canvas, args, overlay_images);

    if bubbles.is_empty() {
        return;
    }

    let t_ms = (frame_id as u64 * 1000) / args.fps as u64;
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

                                let age_secs =
                                    (frame_id.saturating_sub(bubble.spawn_frame)) as f32 / fps_f32;

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
fn frame_signature(
    bubbles: &[Arc<ScheduledMessage>],
    frame_id: u32,
    fps_f32: f32,
    anim_slide: bool,
    anim_fade: bool,
    eviction: &EvictionStrategy,
    hold_secs: f32,
    fade_out_secs: f32,
) -> u64 {
    let mut h = FxHasher::default();
    h.write_usize(bubbles.len());

    let t_ms = (frame_id as u64 * 1000) / fps_f32 as u64;

    for b in bubbles {
        h.write_u32(b.spawn_frame);

        let age = (frame_id.saturating_sub(b.spawn_frame)) as f32 / fps_f32;

        let mut a = 1.0f32;
        if (anim_slide || anim_fade) && age < 0.5 {
            a = age / 0.5;
        }
        if matches!(eviction, EvictionStrategy::Timed) && age > hold_secs {
            a = 1.0 - ((age - hold_secs) / fade_out_secs).clamp(0.0, 1.0);
        }

        // 64 buckets: gives ~8 ms quantisation at 60 fps, perceptually invisible.
        let alpha_bucket = (a * 63.0) as u8;
        h.write_u8(alpha_bucket);

        if anim_slide && age < 0.5 {
            let offset_bucket = (ease_out(age / 0.5) * 63.0) as u8;
            h.write_u8(offset_bucket);
        }

        if b.has_animated_emotes {
            // Bucket GIF t_ms by the message's own period so two messages
            // with different period emotes don't alias each other's signatures.
            h.write_u32(b.gif_frame_index_at(t_ms));
        }
    }

    h.finish()
}

// ──────────────────────────────────────────────────────────────────────────────
// Main render entry point
// ──────────────────────────────────────────────────────────────────────────────

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

    // ── Hardware encoder probe ────────────────────────────────────────────────
    let has_nvenc = Command::new("ffmpeg")
        .args(["-h", "encoder=h264_nvenc"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("h264_nvenc"))
        .unwrap_or(false);

    let is_luma = matches!(args.background_mode, BackgroundMode::LumaMatte);
    let actual_width = if is_luma { args.width * 2 } else { args.width };

    // ── Thread / chunk sizing ─────────────────────────────────────────────────
    // For pipe mode we want to keep latency low so the consumer never stalls,
    // so we use a smaller chunk and fewer threads than standalone mode.
    let (worker_threads, chunk_size, ffmpeg_preset) = if args.use_immediate_pipe_overlay {
        // Pipe mode: minimise frame latency. 1–4 workers, tiny chunks.
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .min(4);
        let preset = if has_nvenc { "p1" } else { "ultrafast" };
        (n, CHUNK_SIZE_BASE, preset)
    } else {
        match args.quality_preset {
            QualityPreset::Draft => (
                1,
                CHUNK_SIZE_BASE,
                if has_nvenc { "p1" } else { "ultrafast" },
            ),
            QualityPreset::Standard => {
                let n = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4)
                    .min(6); // cap to avoid memory pressure from many surfaces
                (
                    n,
                    CHUNK_SIZE_BASE * n,
                    if has_nvenc { "p3" } else { "veryfast" },
                )
            }
            QualityPreset::High => {
                let n = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4)
                    .min(8);
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
    let mut ffmpeg_args = vec!["-y".to_string()];

    if let Some(ref base_video) = args.overlay_video_path {
        if has_nvenc {
            ffmpeg_args.extend(["-hwaccel".into(), "auto".into()]);
        }
        ffmpeg_args.extend([
            "-thread_queue_size".into(),
            "4096".into(),
            "-i".into(),
            base_video.clone(),
        ]);
        ffmpeg_args.extend([
            "-thread_queue_size".into(),
            "4096".into(),
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "bgra".into(), // BGRA matches Skia BGRA8888 output — no conversion needed
            "-s".into(),
            format!("{}x{}", actual_width, args.height),
            "-r".into(),
            args.fps.to_string(),
            "-i".into(),
            "-".into(),
        ]);

        let ox = args.overlay_x.unwrap_or(0);
        let oy = args.overlay_y.unwrap_or(0);
        let eof_action = match args.timeline_mismatch_strategy {
            TimelineMismatchStrategy::RenderClearCanvas => "eof_action=pass",
            _ => "eof_action=repeat",
        };

        let filter_string = if is_luma {
            match (args.overlay_width, args.overlay_height) {
                (Some(ow), Some(oh)) => format!(
                    "[1:v]split=2[c][a]; \
                     [c]crop=w=iw/2:h=ih:x=0:y=0[color]; \
                     [a]crop=w=iw/2:h=ih:x=iw/2:y=0,format=gray[alpha]; \
                     [color][alpha]alphamerge[matte]; \
                     [matte]scale={}:{}[scaled_chat]; \
                     [0:v][scaled_chat]overlay={}:{}:{}[outv]",
                    ow, oh, ox, oy, eof_action
                ),
                _ => format!(
                    "[1:v]split=2[c][a]; \
                     [c]crop=w=iw/2:h=ih:x=0:y=0[color]; \
                     [a]crop=w=iw/2:h=ih:x=iw/2:y=0,format=gray[alpha]; \
                     [color][alpha]alphamerge[overlay_v]; \
                     [0:v][overlay_v]overlay={}:{}:{}[outv]",
                    ox, oy, eof_action
                ),
            }
        } else {
            match (args.overlay_width, args.overlay_height) {
                (Some(ow), Some(oh)) => format!(
                    "[1:v]scale={}:{}[scaled_chat]; \
                     [0:v][scaled_chat]overlay={}:{}:format=yuv420:alpha=premultiplied:{}[outv]",
                    ow, oh, ox, oy, eof_action
                ),
                _ => format!(
                    "[0:v][1:v]overlay={}:{}:format=yuv420:alpha=premultiplied:{}[outv]",
                    ox, oy, eof_action
                ),
            }
        };

        ffmpeg_args.extend([
            "-filter_complex".into(),
            filter_string,
            "-map".into(),
            "[outv]".into(),
            "-map".into(),
            "0:a?".into(),
            "-c:a".into(),
            "copy".into(),
        ]);

        if has_nvenc {
            ffmpeg_args.extend([
                "-c:v".into(),
                "h264_nvenc".into(),
                "-preset".into(),
                ffmpeg_preset.into(),
                "-cq".into(),
                "20".into(),
            ]);
        } else {
            ffmpeg_args.extend([
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
    } else {
        let (vcodec, pix_fmt) = match args.background_mode {
            BackgroundMode::Transparent => ("prores_ks", "yuva444p10le"),
            _ => {
                if has_nvenc {
                    ("h264_nvenc", "yuv420p")
                } else {
                    ("libx264", "yuv420p")
                }
            }
        };
        ffmpeg_args.extend([
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
            ffmpeg_args.extend(["-profile:v".into(), "4444".into()]);
        } else {
            ffmpeg_args.extend([
                "-preset".into(),
                ffmpeg_preset.into(),
                // Provide both -cq and -crf: NVENC uses -cq, libx264 uses -crf.
                // FFmpeg ignores the irrelevant one, so this is safe.
                "-cq".into(),
                "20".into(),
                "-crf".into(),
                "20".into(),
            ]);
        }
    }

    ffmpeg_args.push(args.output_path.clone());

    // ── Spawn FFmpeg ──────────────────────────────────────────────────────────
    let mut ffmpeg_child = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
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
    // Channel for baked ScheduledMessages from the layout thread → render loop.
    let (stamp_tx, stamp_rx) = crossbeam_channel::bounded::<(u32, Arc<ScheduledMessage>)>(2048);

    let scan_path = input_path.clone();
    let scan_cancel = Arc::clone(&cancel_flag);
    let scan_skip = skip_users_set.clone();
    let scan_args = args.clone();
    let scan_emote_map = emote_map.clone();
    let group_window = args.group_messages_window_secs as i64;
    let group_enabled = args.group_messages;
    let time_zero_ms = args.time_zero_ms;

    // Single-shot channel that carries (max_offset_sec, base_ts, emote_ids, image_urls)
    // from the scan thread back to the async task.
    let (meta_tx, meta_rx) = tokio::sync::oneshot::channel::<(f64, i64, Vec<i32>, Vec<String>)>();

    std::thread::spawn(move || {
        let f = match std::fs::File::open(&scan_path) {
            Ok(f) => f,
            Err(_) => return,
        };
        // 2 MiB read buffer: large enough to process multi-MB log files
        // without thrashing the OS page cache.
        let reader = std::io::BufReader::with_capacity(2 << 20, f);

        let mut base_time_secs: Option<i64> = None;
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

            let base = *base_time_secs.get_or_insert_with(|| {
                time_zero_ms
                    .map(|t| (t / 1000) as i64)
                    .unwrap_or(msg.created_at_secs)
            });

            let offset = (msg.created_at_secs - base) as f64;
            if offset > max_offset_sec {
                max_offset_sec = offset;
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
                && (msg.created_at_secs - last_time) <= group_window;
            last_user.clear();
            last_user.push_str(&msg.sender.username);
            last_time = msg.created_at_secs;

            if loader_tx.send((msg, is_grouped)).is_err() {
                break;
            }
        }

        let base = base_time_secs.unwrap_or(0);
        let _ = meta_tx.send((
            max_offset_sec,
            base,
            emote_ids.into_iter().collect(),
            image_urls.into_iter().collect(),
        ));
    });

    let (max_offset_sec, base_time_secs, emote_ids, image_urls) =
        meta_rx.await.unwrap_or((0.0, 0, Vec::new(), Vec::new()));

    emit_progress(5.0, "Hydrating emote caches...");

    // ── Cache warm-up ─────────────────────────────────────────────────────────
    let target_emote_h = ((args.font_size + args.line_spacing as f32) * 1.25).ceil() as u32;

    let emote_cache = Arc::new(EmoteCache::new(
        cache_dir_base.join("emote_cache"),
        args.max_cached_emotes,
        target_emote_h,
        args.quality_preset.clone(),
    ));
    let img_cache = Arc::new(ImageCache::new(
        cache_dir_base.join("image_cache"),
        args.max_cached_emotes,
        target_emote_h * 4, // image URLs are typically larger than emotes
        args.quality_preset.clone(),
    ));

    // Fetch both caches concurrently — network bound, so parallelism helps.
    let (emote_result, img_result) = tokio::join!(
        emote_cache.ensure_cached(&emote_ids),
        img_cache.ensure_cached(&image_urls),
    );
    emote_result?;
    img_result?;

    let overlay_images = Arc::new(load_overlay_images(&args.image_overlays));

    // ── Font setup ────────────────────────────────────────────────────────────
    let font_mgr = FontMgr::new();
    let typeface = font_mgr
        .match_family_style(&args.font_name, FontStyle::normal())
        .or_else(|| font_mgr.match_family_style("Apple Color Emoji", FontStyle::normal()))
        .or_else(|| font_mgr.match_family_style("Segoe UI Emoji", FontStyle::normal()))
        .or_else(|| font_mgr.match_family_style("Noto Color Emoji", FontStyle::normal()))
        .ok_or_else(|| AppError::InternalError("No system fonts found".into()))?;

    let message_font = Font::from_typeface(typeface.clone(), args.font_size);
    let username_font = Font::from_typeface(typeface, (args.font_size * 0.95).max(12.0));
    let (_, metrics) = message_font.metrics();
    let msg_line_h = (metrics.descent - metrics.ascent) + args.line_spacing as f32;

    // ── Layout thread setup ───────────────────────────────────────────────────
    let args_pr = args.clone();
    let emote_cache_pr = Arc::clone(&emote_cache);
    let img_cache_pr = Arc::clone(&img_cache);
    let pr_cancel = Arc::clone(&cancel_flag);
    let render_pool_pr = Arc::clone(&render_pool);
    let highlight_set: FxHashSet<String> = args.pinned_users.iter().cloned().collect();
    let emote_map_pr = emote_map.clone();

    std::thread::spawn(move || {
        // Batch messages before sending them to rayon to amortise per-task
        // scheduling overhead.
        let mut batch: Vec<(MessageSaved, bool)> = Vec::with_capacity(128);
        let mut last_assigned_frame = -1i64;
        let mut layout_gen: u32 = 0;

        let flush_batch = |batch_to_flush: Vec<(MessageSaved, bool)>,
                           last_frame: &mut i64,
                           gen: &mut u32| {
            if batch_to_flush.is_empty() {
                return;
            }
            *gen = gen.wrapping_add(1);
            let current_gen = *gen;

            let rendered: Vec<Option<(i64, ScheduledMessage)>> = render_pool_pr.install(|| {
                batch_to_flush
                    .into_par_iter()
                    .map(|(msg, is_grouped)| {
                        PRE_RENDER_MEASURE_CACHE.with(|cache_cell| {
                            MEASURE_GENERATION.with(|gen_cell| {
                                let mut mc = cache_cell.borrow_mut();
                                let mut tg = gen_cell.borrow_mut();
                                *tg = current_gen;

                                let offset_sec =
                                    ((msg.created_at_secs - base_time_secs) as f64).max(0.0);
                                let base_frame = (offset_sec * args_pr.fps as f64).round() as i64;
                                let is_highlighted = highlight_set.contains(&msg.sender.username);

                                match layout_message_blocking(
                                    &msg.content,
                                    &msg.sender.username,
                                    &msg.sender.identity.color,
                                    &username_font,
                                    &message_font,
                                    (args_pr.width - 2 * args_pr.padding) as f32,
                                    msg_line_h,
                                    metrics.ascent,
                                    &emote_cache_pr,
                                    &img_cache_pr,
                                    &args_pr,
                                    &emote_map_pr,
                                    &mut mc,
                                    current_gen,
                                    is_grouped,
                                ) {
                                    Ok((lines, bubble_w, bubble_h, user_color)) => Some((
                                        base_frame,
                                        ScheduledMessage::new(
                                            0,
                                            lines,
                                            bubble_w,
                                            bubble_h,
                                            Color::from(&args_pr.bubble_color),
                                            user_color,
                                            is_grouped,
                                            is_highlighted,
                                        ),
                                    )),
                                    Err(_) => None,
                                }
                            })
                        })
                    })
                    .collect()
            });

            // Assign spawn frames strictly monotonically so the render loop
            // can do a simple linear scan instead of sorting.
            let mut cursor = *last_frame;
            for (base_frame, mut sched) in rendered.into_iter().flatten() {
                // +2 guard: prevents two consecutive messages from sharing the
                // exact same spawn_frame and appearing simultaneously even when
                // they arrive in the same second of the log.
                let assigned = if base_frame <= cursor {
                    cursor + 2
                } else {
                    base_frame
                };
                cursor = assigned;
                sched.spawn_frame = assigned as u32;
                if stamp_tx.send((sched.spawn_frame, Arc::new(sched))).is_err() {
                    return;
                }
            }
            *last_frame = cursor;
        };

        while let Ok(msg_tuple) = loader_rx.recv() {
            if pr_cancel.load(Ordering::Relaxed) {
                break;
            }
            batch.push(msg_tuple);
            if batch.len() >= 128 {
                flush_batch(
                    std::mem::take(&mut batch),
                    &mut last_assigned_frame,
                    &mut layout_gen,
                );
            }
        }
        // Flush any remaining messages in the last partial batch.
        flush_batch(batch, &mut last_assigned_frame, &mut layout_gen);
    });

    // ── Frame render loop ─────────────────────────────────────────────────────
    let total_frames = ((max_offset_sec * args.fps as f64).round() as u32)
        + (args.message_hold_seconds * args.fps);

    let bg_color = match args.background_mode {
        BackgroundMode::Transparent => Color::TRANSPARENT,
        BackgroundMode::LumaMatte => Color::BLACK,
        BackgroundMode::ChromaKeyGreen => Color::from_argb(255, 0, 255, 0),
        BackgroundMode::CustomColor => Color::from(&args.background_color),
    };

    // BGRA8888 + Premul: matches FFmpeg input format exactly, zero pixel
    // conversion overhead on the CPU side.
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
    let mut frame_chunk: Vec<(u32, Vec<Arc<ScheduledMessage>>)> = Vec::with_capacity(chunk_size);

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
                .retain(|b| (f_idx.saturating_sub(b.spawn_frame)) as f32 / fps_f32 <= max_age);
        }

        // Trim bubbles that scroll off the top of the canvas.
        let mut acc_h = 0i32;
        let mut keep = active_bubbles.len();
        for (i, b) in active_bubbles.iter().enumerate() {
            acc_h += b.bubble_h + args.message_spacing;
            if acc_h >= canvas_max_h {
                keep = i + 2; // keep one extra so scrolling looks smooth
                break;
            }
        }
        if keep < active_bubbles.len() {
            active_bubbles.truncate(keep);
        }

        let vis: Vec<Arc<ScheduledMessage>> = active_bubbles.iter().cloned().collect();
        frame_chunk.push((f_idx, vis));

        if frame_chunk.len() < chunk_size && f_idx < total_frames - 1 {
            continue;
        }

        // ── Dedup: build a list of jobs that actually need rendering ──────────
        // `sequence` maps each frame in the chunk to either a fresh render
        // (Ok(job_index)) or a repeat of `last_buf` (Err(())).
        let mut unique_jobs: Vec<(u32, Vec<Arc<ScheduledMessage>>)> = Vec::new();
        let mut sequence: Vec<Result<usize, ()>> = Vec::with_capacity(frame_chunk.len());

        for (frame_id, bubbles) in &frame_chunk {
            if bubbles.is_empty()
                && args.shape_overlays.is_empty()
                && args.image_overlays.is_empty()
            {
                // Empty canvas: can reuse the previous empty frame.
                let sig = 0u64;
                if last_sig == sig && !prev_animating && last_buf.is_some() {
                    sequence.push(Err(()));
                    prev_animating = false;
                    continue;
                }
                last_sig = sig;
                let job_idx = unique_jobs.len();
                unique_jobs.push((*frame_id, bubbles.clone()));
                sequence.push(Ok(job_idx));
                prev_animating = false;
                continue;
            }

            let is_animating = bubbles.iter().any(|b| {
                b.is_animating(
                    *frame_id, fps_f32, anim_slide, anim_fade, &eviction, hold_secs,
                )
            });

            let sig = frame_signature(
                bubbles, *frame_id, fps_f32, anim_slide, anim_fade, &eviction, hold_secs, fade_secs,
            );

            let dirty = sig != last_sig || last_buf.is_none();

            if dirty {
                let job_idx = unique_jobs.len();
                unique_jobs.push((*frame_id, bubbles.clone()));
                sequence.push(Ok(job_idx));
                last_sig = sig;
            } else {
                sequence.push(Err(()));
            }
            prev_animating = is_animating;
        }

        // ── Parallel render ───────────────────────────────────────────────────
        if !unique_jobs.is_empty() {
            let pool = Arc::clone(&pixel_pool);
            let args_block = args.clone();
            let info_clone = info.clone();
            let ov_imgs_block = Arc::clone(&overlay_images);

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
                                *surf_opt = Some(
                                    surfaces::raster_n32_premul((actual_width, args_block.height))
                                        .unwrap(),
                                );
                            }

                            let surface = surf_opt.as_mut().unwrap();
                            let canvas = surface.canvas();

                            draw_frame(
                                canvas,
                                &bubbles,
                                &args_block,
                                &ov_imgs_block,
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

    // Signal the IO thread that no more frames are coming.
    drop(io_tx);

    if cancel_flag.load(Ordering::SeqCst) {
        let _ = ffmpeg_child.kill();
        let _ = io_thread.join();
        let _ = ffmpeg_child.wait();
        emit_progress(100.0, "Render Cancelled");
        Err(AppError::InternalError("Cancelled by user".into()))
    } else {
        let _ = io_thread.join();
        emit_progress(100.0, "Finishing encoding...");
        let _ = ffmpeg_child.wait();
        emit_progress(100.0, "Complete");
        Ok(())
    }
}
