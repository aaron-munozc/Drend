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
use crate::core::AppTask;
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
use crate::error::AppError;
use crate::types::AppResult;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

fn hidden_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const EMOTE_MARGIN: f32 = 6.0;

/// Chunk sizing: each rayon batch covers 2 frames per worker.
/// Larger = more dedup coalescing; smaller = lower first-frame latency.
const CHUNK_SIZE_MIN: usize = 8;
const CHUNK_SIZE_MAX: usize = 24;

/// Bounded IO channel — backpressure from FFmpeg naturally throttles rendering.
/// Every entry is an `Arc` (8 bytes) not the full pixel buffer.
const IO_CHANNEL_DEPTH: usize = 16;

/// Extra pool headroom above the worker count for scheduling elasticity.
const POOL_HEADROOM: usize = 4;

/// Default RAM budget for pre-allocated pixel buffers.
const DEFAULT_PIXEL_POOL_BUDGET_MB: usize = 384;

/// Hard ceiling on the FFmpeg input queue.
const MAX_FFMPEG_RAW_QUEUE_FRAMES: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// Thread-local caches
//
// All caches are thread-local to eliminate lock contention on the hot render
// path. Each rayon worker thread has its own independent copy; there is zero
// sharing or synchronisation overhead between workers.
// ─────────────────────────────────────────────────────────────────────────────

/// Per-thread text-measure cache capacity.
const MEASURE_CACHE_MAX: usize = 32_768;
type MeasureEntry = (f32, u32); // (advance_width_px, generation)

/// Per-thread username→Color cache capacity.
const USER_COLOR_CACHE_MAX: usize = 512;

/// Per-thread TextBlob cache capacity.
/// TextBlob is an immutable Skia handle; cloning it is O(1) (ref-count bump).
const TEXT_BLOB_CACHE_MAX: usize = 8_192;

thread_local! {
    /// Per-thread Skia raster surface. Created lazily on first use; never
    /// reallocated unless the canvas dimensions change (they don't after init).
    /// Keeping the surface thread-local avoids all locking on the draw path.
    static SKIA_SURFACE: RefCell<Option<skia_safe::Surface>> = RefCell::new(None);

    /// (text_hash ++ font_size_bits) → (advance_width, generation)
    static PRE_RENDER_MEASURE_CACHE: RefCell<FxHashMap<u64, MeasureEntry>> =
        RefCell::new(FxHashMap::with_capacity_and_hasher(4096, Default::default()));

    /// Monotonic generation counter for bulk eviction of stale measure entries.
    static MEASURE_GENERATION: RefCell<u32> = RefCell::new(0);

    /// username_hash → Color — avoids repeated hex→Color parsing.
    static USER_COLOR_CACHE: RefCell<FxHashMap<u64, Color>> =
        RefCell::new(FxHashMap::with_capacity_and_hasher(64, Default::default()));

    /// (text_hash ++ font_bits) → TextBlob — avoids repeated text shaping.
    /// Populated once per unique (text, font) pair; cloning the handle is free.
    static TEXT_BLOB_CACHE: RefCell<FxHashMap<u64, TextBlob>> =
        RefCell::new(FxHashMap::with_capacity_and_hasher(1024, Default::default()));

    /// Per-thread exact-message layout cache.
    /// Keyed on (content, username, color, is_grouped) so identical messages
    /// across different senders (a common spam pattern) share layout work.
    static LAYOUT_CACHE: RefCell<FxHashMap<u64, CachedLayout>> =
        RefCell::new(FxHashMap::with_capacity_and_hasher(512, Default::default()));

    // ── Pre-allocated Paint objects ────────────────────────────────────────────
    // Constructing a Paint object involves heap allocation for its filter chain.
    // Reusing them across draw_frame calls eliminates that per-frame allocation.

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

// ─────────────────────────────────────────────────────────────────────────────
// Pixel buffer pool
//
// Pre-allocated pool of reusable BGRA pixel buffers.
//
// Design goals:
//   1. Zero per-frame heap allocation on the hot render path.
//   2. Bounded size — never grows beyond `max_buffers` regardless of core count.
//   3. O(1) acquire and release — single lock on a Vec<Vec<u8>>.
//   4. RAII: `ReusableBuffer` returns the buffer to the pool on drop.
//
// The pool is byte-budgeted (derived from `render_memory_budget_mb` and the
// actual frame size) so a 4K luma-matte job doesn't allocate as many buffers
// as a 400×800 job just because the machine has many CPU cores.
// ─────────────────────────────────────────────────────────────────────────────

struct PixelBufferPool {
    inner: Mutex<Vec<Vec<u8>>>,
    max_buffers: usize,
}

impl PixelBufferPool {
    fn new(max_buffers: usize) -> Self {
        Self { inner: Mutex::new(Vec::with_capacity(max_buffers)), max_buffers }
    }

    fn acquire(&self, min_len: usize) -> Vec<u8> {
        let mut guard = self.inner.lock().unwrap();
        if let Some(mut buf) = guard.pop() {
            drop(guard);
            if buf.capacity() < min_len {
                buf.reserve_exact(min_len - buf.capacity());
            }
            // SAFETY: caller (Skia read_pixels) will overwrite every byte.
            unsafe { buf.set_len(min_len) };
            return buf;
        }
        drop(guard);
        let mut buf = Vec::with_capacity(min_len);
        // SAFETY: same contract as above.
        unsafe { buf.set_len(min_len) };
        buf
    }

    fn release(&self, mut buf: Vec<u8>) {
        // Reset length so capacity is preserved but contents are uninitialised.
        unsafe { buf.set_len(0) };
        let mut g = self.inner.lock().unwrap();
        if g.len() < self.max_buffers {
            g.push(buf);
        }
        // Otherwise the buffer is dropped here — pool is at capacity.
    }
}

/// RAII wrapper: returns the pixel buffer to the pool on drop.
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
        Self { data: Some(pool.acquire(len)), pool }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ScheduledMessage — a fully-laid-out chat bubble ready for the draw loop
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct ScheduledMessage {
    spawn_frame: u32,
    /// Pre-existing age at frame 0 for prefill messages; 0 for normal messages.
    age_offset_frames: u32,
    /// Pre-computed layout lines (TextBlobs + emote handles). Never mutated.
    lines: Vec<LayoutLine>,
    bubble_w: i32,
    bubble_h: i32,
    bg_color: Color,
    user_color: Color,
    is_grouped: bool,
    /// Stable visual identity hash — avoids comparing Arc addresses.
    visual_key: u64,
    /// True when any token in `lines` is an animated emote.
    has_animated_emotes: bool,
    is_highlighted: bool,
    /// Shortest animated-emote cycle (ms), or `None` if all tokens are static.
    anim_period_ms: Option<u32>,
    /// Cumulative pixel height of visible lines, used for viewport culling.
    /// Computed once at construction; never re-derived per frame.
    cumulative_h: i32,
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
        visual_key: u64,
    ) -> Self {
        Self::new_inner(spawn_frame, 0, lines, bubble_w, bubble_h, bg_color, user_color, is_grouped, is_highlighted, visual_key)
    }

    fn new_prefill(
        age_offset_frames: u32,
        lines: Vec<LayoutLine>,
        bubble_w: i32,
        bubble_h: i32,
        bg_color: Color,
        user_color: Color,
        is_grouped: bool,
        is_highlighted: bool,
        visual_key: u64,
    ) -> Self {
        Self::new_inner(0, age_offset_frames, lines, bubble_w, bubble_h, bg_color, user_color, is_grouped, is_highlighted, visual_key)
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
        mut visual_key: u64,
    ) -> Self {
        if is_highlighted { visual_key ^= 0x9E37_79B9_7F4A_7C15; }
        let mut has_animated_emotes = false;
        let mut anim_period_ms: Option<u32> = None;

        for l in &lines {
            for t in &l.tokens {
                if let LayoutToken::Emote { data, .. } = t {
                    match data.as_ref() {
                        EmoteData::Animated { total_ms, .. }
                        | EmoteData::LazyGif { total_ms, .. } => {
                            has_animated_emotes = true;
                            anim_period_ms = Some(match anim_period_ms {
                                None => *total_ms,
                                Some(p) => p.min(*total_ms),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }

        Self {
            spawn_frame,
            age_offset_frames,
            cumulative_h: bubble_h,
            lines,
            bubble_w,
            bubble_h,
            bg_color,
            user_color,
            is_grouped,
            visual_key,
            has_animated_emotes,
            is_highlighted,
            anim_period_ms,
        }
    }

    /// Effective age of this bubble at `frame_id`, in seconds.
    #[inline(always)]
    fn effective_age(&self, frame_id: u32, fps: f32) -> f32 {
        let on_screen = frame_id.saturating_sub(self.spawn_frame);
        (on_screen + self.age_offset_frames) as f32 / fps
    }

    /// GIF playback offset in ms at time `t_ms`.
    #[inline(always)]
    fn gif_frame_index_at(&self, t_ms: u64) -> u32 {
        match self.anim_period_ms {
            None | Some(0) => 0,
            Some(period) => (t_ms % period as u64) as u32,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Measure / TextBlob cache helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn measure_key(s: &str, font_size_bits: u32) -> u64 {
    let mut h = FxHasher::default();
    h.write(s.as_bytes());
    h.write_u32(font_size_bits);
    h.finish()
}

/// Bulk-evict stale measure entries. Cheaper than LRU because we only need
/// generational (not per-entry) recency.
fn evict_old_measure_entries(cache: &mut FxHashMap<u64, MeasureEntry>, current_gen: u32) {
    cache.retain(|_, (_, gen)| current_gen.saturating_sub(*gen) <= 1);
    if cache.len() > MEASURE_CACHE_MAX {
        cache.retain(|_, (_, gen)| *gen == current_gen);
    }
}

#[inline(always)]
fn get_user_color_cached(username: &str, hex_color: &str) -> Color {
    let key = {
        let mut h = FxHasher::default();
        h.write(username.as_bytes());
        h.write_u8(0xFF);
        h.write(hex_color.as_bytes());
        h.finish()
    };
    USER_COLOR_CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some(&c) = cache.get(&key) { return c; }
        let color = get_user_color(username, hex_color);
        if cache.len() >= USER_COLOR_CACHE_MAX {
            let mut keep = false;
            cache.retain(|_, _| { keep = !keep; keep });
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
    if let Some(&(w, _)) = cache.get(&k) { return w; }
    if cache.len() >= MEASURE_CACHE_MAX { evict_old_measure_entries(cache, gen); }
    let (w, _) = font.measure_str(s, None);
    cache.insert(k, (w, gen));
    w
}

/// Return a cached `TextBlob` when available.
///
/// `TextBlob` is an immutable Skia handle — cloning it is a reference-count
/// bump (O(1)), not a copy of the glyph data. This eliminates repeated text
/// shaping for the same (text, font) pair, which is the dominant CPU cost
/// for static message rendering.
///
/// Eviction retains a random half rather than nuking everything, so hot blobs
/// (usernames, common words) survive overflow instead of being re-shaped on the
/// very next frame.
#[inline(always)]
fn text_blob_cached(s: &str, font: &Font, font_bits: u32) -> Option<TextBlob> {
    let key = measure_key(s, font_bits);
    TEXT_BLOB_CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some(blob) = cache.get(&key) { return Some(blob.clone()); }
        let blob = TextBlob::from_str(s, font)?;
        if cache.len() >= TEXT_BLOB_CACHE_MAX {
            let mut keep = false;
            cache.retain(|_, _| { keep = !keep; keep });
        }
        cache.insert(key, blob.clone());
        Some(blob)
    })
}

/// Split `input` into substrings that each fit within `max_w` pixels.
///
/// Uses binary search over character byte-offsets to minimise `measure_str`
/// calls. Char offsets are collected into a plain `Vec` so arbitrarily long
/// tokens (spam URLs, base64 blobs, etc.) never exceed a fixed capacity and
/// cannot panic inside a Rayon worker.
///
/// # Correctness guarantee
///
/// Every returned slice is non-empty: when the binary search fails to find
/// even a one-character fit (e.g. `max_w` is narrower than a single glyph),
/// we emit exactly one character and advance past it. This ensures the outer
/// loop always makes forward progress and terminates.
fn split_into_fragments<'a>(
    input: &'a str,
    font: &Font,
    font_bits: u32,
    max_w: f32,
    cache: &mut FxHashMap<u64, MeasureEntry>,
    gen: u32,
) -> Vec<&'a str> {
    // Typical chat tokens are short; pre-size for the common case.
    let mut out: Vec<&'a str> = Vec::with_capacity(8);
    let mut start = 0usize;

    while start < input.len() {
        let remainder = &input[start..];

        // Fast path: remainder already fits — no binary search needed.
        if measure_cached(font, font_bits, remainder, cache, gen) <= max_w {
            out.push(remainder);
            break;
        }

        // Collect char byte-offsets lazily into a plain Vec.
        // No fixed capacity — handles tokens of any length.
        let char_offsets: Vec<usize> = remainder.char_indices().map(|(i, _)| i).collect();
        let char_count = char_offsets.len();

        // Single-char (or empty) remainder: emit it whole and stop.
        if char_count <= 1 {
            out.push(remainder);
            break;
        }

        // Binary search for the longest prefix that fits within max_w.
        let mut lo = 1usize;
        let mut hi = char_count - 1;
        let mut best_byte = 0usize;

        while lo <= hi {
            let mid = (lo + hi) / 2;
            // mid is always in [1, char_count-1], so char_offsets[mid] is valid.
            let byte_off = char_offsets[mid];
            if measure_cached(font, font_bits, &remainder[..byte_off], cache, gen) <= max_w {
                best_byte = byte_off;
                lo = mid + 1;
            } else {
                hi = mid.saturating_sub(1);
                if mid == 0 { break; }
            }
        }

        // best_byte == 0 means not even the first character fits.
        // Emit exactly one character so we always make forward progress.
        let slice_end = if best_byte == 0 {
            char_offsets.get(1).copied().unwrap_or(remainder.len())
        } else {
            best_byte
        };

        out.push(&remainder[..slice_end]);
        start += slice_end;
    }

    out
}

#[derive(Clone)]
struct CachedLayout {
    lines: Vec<LayoutLine>,
    width: i32,
    height: i32,
    user_color: Color,
    /// Layout batch generation at insertion time. Used by generational eviction
    /// to retain recently-computed entries and discard stale ones — same pattern
    /// as `PRE_RENDER_MEASURE_CACHE`, which already uses this approach.
    gen: u32,
}

/// Generational eviction for the per-thread layout cache.
///
/// Retains entries from the current and immediately prior generation, then
/// hard-caps at `max`. This is O(n) in entries but runs only when the cache
/// is full — typically once per batch on spam-heavy logs. The measure cache
/// uses the same strategy.
fn evict_old_layout_entries(cache: &mut FxHashMap<u64, CachedLayout>, current_gen: u32) {
    cache.retain(|_, v| current_gen.saturating_sub(v.gen) <= 1);
    if cache.len() > 512 {
        cache.retain(|_, v| v.gen == current_gen);
    }
}

/// Stable key for exact-message layout deduplication.
///
/// Hashes every input that can change the baked layout: content, username,
/// color, and grouping state. Two messages that differ only in timestamp but
/// are otherwise identical will share the same layout result.
#[inline]
fn message_layout_key(msg: &MessageSaved, is_grouped: bool) -> u64 {
    let mut h = FxHasher::default();
    h.write(msg.content.as_bytes());
    h.write_u8(0xFF);
    h.write(msg.sender.username.as_bytes());
    h.write_u8(0xFE);
    h.write(msg.sender.identity.color.as_bytes());
    h.write_u8(is_grouped as u8);
    h.finish()
}

// ─────────────────────────────────────────────────────────────────────────────
// layout_message_blocking
//
// Converts a raw message string + metadata into a fully laid-out `Vec<LayoutLine>`
// ready for the draw loop. This is the heaviest per-message computation:
//   - Text shaping (TextBlob construction via Skia HarfBuzz)
//   - Word-wrap (binary search per fragment)
//   - Emote-cache lookup and placement
//
// Results are cached in the per-thread LAYOUT_CACHE so repeated messages
// (spam, identical greetings) pay only one layout cost per thread.
// ─────────────────────────────────────────────────────────────────────────────

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

    let flags = &args.emote_providers;
    let map_opt = if !emote_map.is_empty() && flags.any_name_provider_enabled() {
        Some((emote_map, flags))
    } else {
        None
    };
    let tokens = tokenise(content, map_opt);
    let max_w = available_w.max(1.0);

    // Build the username prefix on the stack — avoids a heap allocation for
    // the common case where usernames are ≤ 94 bytes.
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
                if s.chars().all(|c| c.is_whitespace()) {
                    if !current_line.is_empty() {
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
                        let word_w = measure_cached(message_font, mf_bits, raw_word, measure_cache, gen);
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
                                let frags = split_into_fragments(raw_word, message_font, mf_bits, max_w, measure_cache, gen);
                                let flen = frags.len();
                                for (fi, f) in frags.into_iter().enumerate() {
                                    current_line.push(MessageToken::Text(f));
                                    cur_w += measure_cached(message_font, mf_bits, f, measure_cache, gen);
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
                if !flags.kick { continue; }
                let ew = emote_cache
                    .get(*id)
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
            MessageToken::ProviderEmote(ResolvedEmote { url, zero_width, .. }) => {
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
        }
    }
    if !current_line.is_empty() { lines.push(current_line); }

    // ── Measure + bake TextBlobs ──────────────────────────────────────────────
    let bubble_pad = args.bubble_padding.max(0) as f32;
    let mut layout_lines = Vec::with_capacity(lines.len());
    let mut measured_max_w = 0f32;
    let mut y_cursor = bubble_pad;

    for (li, line) in lines.iter().enumerate() {
        // Line height is the max of text height and emote height.
        let mut lh = msg_line_h;
        for token in line {
            match token {
                MessageToken::Text(_) => {}
                MessageToken::KickEmote { id } => {
                    let h = emote_cache
                        .get(*id)
                        .map(|ed| ed.height() as f32)
                        .unwrap_or(emote_cache.target_height() as f32);
                    lh = lh.max(h);
                }
                MessageToken::ProviderEmote(ResolvedEmote { url, zero_width, .. }) => {
                    let h = image_cache.get(url).map(|ed| ed.height() as f32)
                                       .unwrap_or(image_cache.target_height() as f32);
                    if !zero_width { lh = lh.max(h + 8.0); }
                }
            }
        }

        // Baseline: bottom of text glyphs within the (potentially taller) line box.
        let baseline = y_cursor + ((lh - msg_line_h) / 2.0).max(0.0) - message_ascent;
        let mut x_cursor = bubble_pad;
        let mut layout_tokens: Vec<LayoutToken> = Vec::with_capacity(line.len() + 1);

        // First line: username prefix (unless grouped).
        if li == 0 && !is_grouped {
            if let Some(blob) = text_blob_cached(prefix_str, username_font, uf_bits) {
                layout_tokens.push(LayoutToken::Glyph { blob, x: x_cursor, y: baseline });
            }
            x_cursor += prefix_w;
        }

        for token in line {
            match token {
                MessageToken::Text(s) => {
                    let w = measure_cached(message_font, mf_bits, s, measure_cache, gen);
                    if let Some(blob) = text_blob_cached(s, message_font, mf_bits) {
                        layout_tokens.push(LayoutToken::Glyph { blob, x: x_cursor, y: baseline });
                    }
                    x_cursor += w;
                }
                MessageToken::KickEmote { id } => {
                    let fallback_w = emote_cache.target_height() as f32;
                    if let Some(ed) = emote_cache.get(*id) {
                        let ew = ed.width() as f32;
                        let draw_y = if args.center_emotes_vertically {
                            y_cursor + (lh - ed.height() as f32) / 2.0
                        } else { y_cursor };
                        layout_tokens.push(LayoutToken::Emote {
                            data: ed, x: x_cursor + (EMOTE_MARGIN / 2.0), y: draw_y,
                        });
                        x_cursor += ew + EMOTE_MARGIN;
                    } else {
                        x_cursor += fallback_w + EMOTE_MARGIN;
                    }
                }
                MessageToken::ProviderEmote(ResolvedEmote { url, zero_width, .. }) => {
                    let fallback_w = image_cache.target_height() as f32;
                    if let Some(ed) = image_cache.get(url) {
                        let sw = ed.width() as f32;
                        let target_x = if *zero_width && x_cursor > bubble_pad {
                            x_cursor - sw - (EMOTE_MARGIN / 2.0)
                        } else { x_cursor + (EMOTE_MARGIN / 2.0) };
                        let draw_y = if args.center_emotes_vertically {
                            y_cursor + (lh - ed.height() as f32) / 2.0
                        } else { y_cursor };
                        layout_tokens.push(LayoutToken::Emote { data: ed, x: target_x, y: draw_y });
                        if !zero_width { x_cursor += sw + EMOTE_MARGIN; }
                    } else if !zero_width { x_cursor += fallback_w + EMOTE_MARGIN; }
                }
            }
        }

        measured_max_w = measured_max_w.max(x_cursor - bubble_pad);
        layout_lines.push(LayoutLine { tokens: layout_tokens });
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

// ─────────────────────────────────────────────────────────────────────────────
// draw_frame — core Skia rasterisation
//
// Optimisations applied here:
//   1. All Paint objects are pre-allocated in thread-local storage — zero
//      allocation per frame. A single `with` block borrows all six paints
//      at once to avoid nested RefCell overhead.
//   2. Viewport culling: bubbles are drawn bottom-up (newest at bottom);
//      once `y_cursor` goes negative we stop — no invisible bubble is touched.
//   3. Alpha short-circuit: bubbles with alpha ≤ 0 skip all draw calls
//      and only update `y_cursor`.
//   4. Animated-emote frame selection: O(log n) binary search on pre-sorted
//      cumulative-duration slice; no runtime GIF decoding per frame.
//   5. Dirty-rect clipping (optional, interactive mode only): when only
//      animated emotes changed, `canvas.clip_rect` limits redraws to those
//      bounding boxes, leaving the static portions untouched.
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_frame(
    canvas: &skia_safe::Canvas,
    bubbles: &[Arc<ScheduledMessage>],
    bg_color: Color,
    is_luma: bool,
    frame_id: u32,
    fps_f32: f32,
    hold_secs: f32,
    fade_out_f: f32,
    anim_slide: bool,
    anim_fade: bool,
    eviction: &EvictionStrategy,
    // Pre-computed constants — derived from RenderVideoArgs once per job.
    msg_color: Color,
    hi_color: Color,
    outline_w: f32,
    y_start: f32,
    padding_f: f32,
    spacing_f: f32,
    canvas_width_f: f32,
    bubble_radius: f32,
    username_shadow: bool,
    outline_usernames: bool,
) {
    canvas.clear(bg_color);
    if bubbles.is_empty() { return; }

    let t_ms = ((frame_id as f64 * 1000.0) / fps_f32 as f64) as u64;

    // Start at the bottom of the canvas and draw upward.
    let mut y_cursor = y_start;

    PAINT_BG.with(|pb| {
        PAINT_HIGHLIGHT.with(|ph| {
            PAINT_MASK_BG.with(|pm| {
                PAINT_TEXT.with(|pt| {
                    PAINT_EMOTE.with(|pe| {
                        PAINT_EMOTE_MASK.with(|pem| {
                            let mut paint_bg       = pb.borrow_mut();
                            let mut paint_highlight = ph.borrow_mut();
                            let mut mask_bg        = pm.borrow_mut();
                            let mut text_paint     = pt.borrow_mut();
                            let mut emote_paint    = pe.borrow_mut();
                            let mut emote_mask_paint = pem.borrow_mut();

                            for bubble in bubbles {
                                // ── Viewport culling ──────────────────────────
                                // y_cursor is the bottom edge of the next bubble.
                                // If it's already above the canvas top we're done.
                                if y_cursor < 0.0 { break; }

                                let age_secs = bubble.effective_age(frame_id, fps_f32);

                                // Per-bubble alpha (fade-in × fade-out).
                                let alpha = {
                                    let mut a = 1.0f32;
                                    if anim_fade && age_secs < 0.5 {
                                        a *= age_secs / 0.5;
                                    }
                                    if matches!(eviction, EvictionStrategy::Timed) && age_secs > hold_secs {
                                        a *= 1.0 - ((age_secs - hold_secs) / fade_out_f).clamp(0.0, 1.0);
                                    }
                                    a
                                };

                                // Skip fully invisible bubbles — no Skia calls.
                                if alpha <= 0.0 {
                                    y_cursor -= bubble.bubble_h as f32 + spacing_f;
                                    continue;
                                }

                                let byte_alpha = (255.0 * alpha) as u8;
                                let top = y_cursor - bubble.bubble_h as f32;

                                let x_translate = if anim_slide && age_secs < 0.5 {
                                    padding_f + (1.0 - ease_out(age_secs / 0.5)) * (canvas_width_f - padding_f)
                                } else {
                                    padding_f
                                };

                                let bw = bubble.bubble_w as f32;
                                let bh = bubble.bubble_h as f32;
                                let rect = Rect::new(0.0, 0.0, bw, bh);

                                // ── Colour pass ───────────────────────────────
                                canvas.save();
                                canvas.translate((x_translate, top));

                                paint_bg.set_color(bubble.bg_color.with_a(byte_alpha));
                                canvas.draw_round_rect(rect, bubble_radius, bubble_radius, &paint_bg);

                                if bubble.is_highlighted {
                                    paint_highlight.set_color(hi_color.with_a(byte_alpha));
                                    canvas.draw_round_rect(rect, bubble_radius, bubble_radius, &paint_highlight);
                                }

                                if is_luma {
                                    // Mask background drawn first so it sits behind tokens.
                                    // Uses a temporary translate rather than a full save/restore
                                    // since the canvas state (x_translate, top) is still active.
                                    canvas.save();
                                    canvas.translate((canvas_width_f, 0.0));
                                    mask_bg.set_color(Color::WHITE.with_a(byte_alpha));
                                    canvas.draw_round_rect(rect, bubble_radius, bubble_radius, &mask_bg);
                                    canvas.restore();

                                    // Single-pass combined draw: colour left half + mask right half
                                    // in one token-list walk. Halves token-iteration cost vs the
                                    // previous two-pass approach for the default luma-matte mode.
                                    draw_bubble_tokens_luma(
                                        canvas, bubble,
                                        &mut text_paint, &mut emote_paint, &mut emote_mask_paint,
                                        t_ms, alpha, byte_alpha, msg_color,
                                        outline_w, canvas_width_f,
                                        username_shadow, outline_usernames,
                                    );
                                } else {
                                    draw_bubble_tokens(
                                        canvas, bubble, &mut text_paint, &mut emote_paint,
                                        t_ms, alpha, byte_alpha, msg_color,
                                        outline_w, username_shadow, outline_usernames,
                                        false,
                                    );
                                }
                                canvas.restore();

                                y_cursor -= bh + spacing_f;
                            }
                        })
                    })
                })
            })
        })
    });
}

/// Draw all tokens within a single chat bubble — colour output only (non-luma path).
///
/// All arguments that were previously read from `&RenderVideoArgs` are now
/// passed as pre-computed scalars so the function body has zero field-dereference
/// overhead per call.
///
/// Animated emote frame selection uses `EmoteData::frame_at(t_ms)`:
/// O(log n) binary search on the pre-sorted cumulative-duration slice.
/// No heap allocation; no GIF decode at render time.
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
    outline_w: f32,
    username_shadow: bool,
    outline_usernames: bool,
    is_mask: bool,
) {
    for (li, line) in bubble.lines.iter().enumerate() {
        for (ti, token) in line.tokens.iter().enumerate() {
            match token {
                LayoutToken::Glyph { blob, x, y } => {
                    let is_username = !bubble.is_grouped && li == 0 && ti == 0;
                    if is_mask {
                        text_paint.set_color(Color::from_argb(byte_alpha, 255, 255, 255));
                        canvas.draw_text_blob(blob, (*x, *y), text_paint);
                    } else {
                        let base_color = if is_username { bubble.user_color } else { msg_color };
                        let final_color = base_color.with_a(
                            (base_color.a() as f32 * alpha).min(255.0) as u8,
                        );
                        if is_username && byte_alpha > 5 {
                            if username_shadow {
                                text_paint.set_color(Color::from_argb(
                                    (180.0 * alpha) as u8, 0, 0, 0,
                                ));
                                canvas.draw_text_blob(blob, (*x + 2.0, *y + 2.0), text_paint);
                            }
                            if outline_usernames {
                                text_paint.set_style(skia_safe::paint::Style::Stroke);
                                text_paint.set_stroke_width(outline_w);
                                text_paint.set_color(Color::from_argb(
                                    (200.0 * alpha) as u8, 0, 0, 0,
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
                        let dest = Rect::new(
                            *x, *y,
                            *x + data.width() as f32,
                            *y + data.height() as f32,
                        );
                        emote_paint.set_alpha(byte_alpha);
                        canvas.draw_image_rect(img, None, dest, emote_paint);
                    }
                }
            }
        }
    }
}

/// Luma-matte single-pass combined draw.
///
/// Iterates the token list **once** and emits both the colour draw call (at
/// the token's stored coordinates) and the mask draw call (at `x + half_w`)
/// in the same loop body. This halves token-iteration cost compared to the
/// previous two-pass approach where `draw_bubble_tokens` was called twice per
/// bubble. The background mask rectangle is drawn separately by the caller
/// before this function runs so the rect draw remains independent.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn draw_bubble_tokens_luma(
    canvas: &skia_safe::Canvas,
    bubble: &ScheduledMessage,
    text_paint: &mut Paint,
    emote_paint: &mut Paint,
    emote_mask_paint: &mut Paint,
    t_ms: u64,
    alpha: f32,
    byte_alpha: u8,
    msg_color: Color,
    outline_w: f32,
    half_w: f32,
    username_shadow: bool,
    outline_usernames: bool,
) {
    let mask_white = Color::from_argb(byte_alpha, 255, 255, 255);

    for (li, line) in bubble.lines.iter().enumerate() {
        for (ti, token) in line.tokens.iter().enumerate() {
            match token {
                LayoutToken::Glyph { blob, x, y } => {
                    let is_username = !bubble.is_grouped && li == 0 && ti == 0;

                    // ── colour draw ───────────────────────────────────────────
                    let base_color = if is_username { bubble.user_color } else { msg_color };
                    let final_color = base_color.with_a(
                        (base_color.a() as f32 * alpha).min(255.0) as u8,
                    );
                    if is_username && byte_alpha > 5 {
                        if username_shadow {
                            text_paint.set_color(Color::from_argb(
                                (180.0 * alpha) as u8, 0, 0, 0,
                            ));
                            canvas.draw_text_blob(blob, (*x + 2.0, *y + 2.0), text_paint);
                        }
                        if outline_usernames {
                            text_paint.set_style(skia_safe::paint::Style::Stroke);
                            text_paint.set_stroke_width(outline_w);
                            text_paint.set_color(Color::from_argb(
                                (200.0 * alpha) as u8, 0, 0, 0,
                            ));
                            canvas.draw_text_blob(blob, (*x, *y), text_paint);
                            text_paint.set_style(skia_safe::paint::Style::Fill);
                        }
                    }
                    text_paint.set_color(final_color);
                    canvas.draw_text_blob(blob, (*x, *y), text_paint);

                    // ── mask draw (same blob, offset X) ──────────────────────
                    text_paint.set_color(mask_white);
                    canvas.draw_text_blob(blob, (*x + half_w, *y), text_paint);
                }
                LayoutToken::Emote { data, x, y } => {
                    if let Some(img) = data.frame_at(t_ms) {
                        let w = data.width() as f32;
                        let h = data.height() as f32;

                        // colour pass
                        let colour_dest = Rect::new(*x, *y, *x + w, *y + h);
                        emote_paint.set_alpha(byte_alpha);
                        canvas.draw_image_rect(img, None, colour_dest, emote_paint);

                        // mask pass — same image, offset X by half_w
                        let mask_dest = Rect::new(*x + half_w, *y, *x + half_w + w, *y + h);
                        emote_mask_paint.set_alpha(byte_alpha);
                        canvas.draw_image_rect(img, None, mask_dest, emote_mask_paint);
                    }
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Frame-signature hash
//
// Produces a hash that changes exactly when the visual output changes.
//
// Key design decisions:
//   1. Operates directly on `&VecDeque<Arc<ScheduledMessage>>` — no clone.
//   2. Alpha and slide-offset values are bucketed into 64 discrete steps so
//      slow animations produce long runs of identical signatures. This lets
//      the render loop coalesce many consecutive frames into a single render
//      call, cutting CPU load dramatically for static chat periods.
//   3. GIF frame selection uses the same `gif_frame_index_at` function as the
//      draw pass, so the signature and the frame are always in sync.
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
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
        h.write_u64(b.visual_key);
        h.write_u32(b.spawn_frame);
        h.write_u32(b.age_offset_frames);
        let age = b.effective_age(frame_id, fps_f32);
        let mut a = 1.0f32;
        if (anim_slide || anim_fade) && age < 0.5 { a = age / 0.5; }
        if matches!(eviction, EvictionStrategy::Timed) && age > hold_secs {
            a = 1.0 - ((age - hold_secs) / fade_secs).clamp(0.0, 1.0);
        }
        h.write_u8((a * 63.0) as u8);
        if anim_slide && age < 0.5 {
            h.write_u8((ease_out(age / 0.5) * 63.0) as u8);
        }
        if b.has_animated_emotes {
            h.write_u32(b.gif_frame_index_at(t_ms));
        }
    }
    h.finish()
}

// ─────────────────────────────────────────────────────────────────────────────
// FFmpeg helpers
// ─────────────────────────────────────────────────────────────────────────────

fn probe_video_frames(path: &str, fps: u32) -> Option<u32> {
    let try_probe = |show_entries: &str| -> Option<f64> {
        let out = hidden_command("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries",
                show_entries, "-of", "default=noprint_wrappers=1:nokey=1", path])
            .output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let line = s.lines().find(|l| !l.trim().is_empty())?;
        line.trim().parse().ok()
    };
    let duration_secs = try_probe("stream=duration").or_else(|| try_probe("format=duration"))?;
    let frames = ((duration_secs + 3.0) * fps as f64).ceil() as u32;
    Some(frames)
}

fn probe_nvenc() -> bool {
    hidden_command("ffmpeg")
        .args(["-h", "encoder=h264_nvenc"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("h264_nvenc"))
        .unwrap_or(false)
}

#[inline]
fn ffmpeg_encode_threads(cpus: usize, worker_threads: usize, has_nvenc: bool) -> usize {
    if has_nvenc { 1 } else { cpus.saturating_sub(worker_threads).saturating_sub(1).clamp(1, 2) }
}

/// Convert a RAM budget into a bounded pixel-buffer count.
///
/// The count is derived from the actual frame size so 4K luma-matte jobs
/// don't accidentally allocate hundreds of buffers just because the machine
/// has many cores.
#[inline]
fn pixel_pool_buffer_count(frame_bytes: usize, worker_threads: usize, budget_mb: Option<usize>) -> usize {
    let budget = budget_mb.unwrap_or(DEFAULT_PIXEL_POOL_BUDGET_MB).clamp(64, 4096) * 1024 * 1024;
    if frame_bytes == 0 { return worker_threads + IO_CHANNEL_DEPTH + POOL_HEADROOM; }
    let frame_capacity = (budget / frame_bytes).max(1);
    let floor = (worker_threads + POOL_HEADROOM).min(frame_capacity);
    frame_capacity.max(floor).min(128)
}

fn build_overlay_ffmpeg_args(
    args: &RenderVideoArgs,
    actual_width: i32,
    is_luma: bool,
    has_nvenc: bool,
    ffmpeg_preset: &str,
    encode_threads: usize,
) -> Vec<String> {
    let mut a = vec!["-y".to_string()];
    if has_nvenc { a.extend(["-hwaccel".into(), "auto".into()]); }
    let base_video = args.overlay_video_path.as_ref().unwrap();
    a.extend(["-thread_queue_size".into(), "512".into(), "-i".into(), base_video.clone()]);
    a.extend([
        "-thread_queue_size".into(),
        args.ffmpeg_input_queue_frames.unwrap_or(16).clamp(4, MAX_FFMPEG_RAW_QUEUE_FRAMES).to_string(),
        "-f".into(), "rawvideo".into(), "-pix_fmt".into(), "bgra".into(),
        "-s".into(), format!("{}x{}", actual_width, args.height),
        "-r".into(), args.fps.to_string(), "-i".into(), "-".into(),
    ]);
    let img_input_start = 2usize;
    for ov in &args.image_overlays {
        a.extend(["-thread_queue_size".into(), "64".into(), "-loop".into(), "1".into(),
            "-i".into(), ov.asset_path.clone()]);
    }
    let ox = args.overlay_x.unwrap_or(0);
    let oy = args.overlay_y.unwrap_or(0);
    let eof_action = match args.timeline_mismatch_strategy {
        TimelineMismatchStrategy::RenderClearCanvas => "eof_action=pass",
        _ => "eof_action=repeat",
    };

    let mut filter_parts: Vec<String> = Vec::new();
    let mut current_base = "[0:v]".to_string();
    let mut label_idx = 0usize;

    for shape in &args.shape_overlays {
        let next_label = format!("[base{}]", label_idx);
        label_idx += 1;
        let r = shape.color.red.clamp(0, 255) as u8;
        let g = shape.color.green.clamp(0, 255) as u8;
        let b = shape.color.blue.clamp(0, 255) as u8;
        let alpha_f = shape.color.alpha.clamp(0, 255) as f32 / 255.0;
        let shape_filter = format!(
            "color=c=0x{:02X}{:02X}{:02X}@{:.4}:s={}x{}:r={},setpts=PTS-STARTPTS[shape{}]; \
             {}[shape{}]overlay={}:{}:format=auto{}",
            r, g, b, alpha_f,
            shape.width as u32, shape.height as u32, args.fps, label_idx - 1,
            current_base, label_idx - 1, shape.x as i32, shape.y as i32, next_label
        );
        filter_parts.push(shape_filter);
        current_base = next_label;
    }

    for (i, ov) in args.image_overlays.iter().enumerate() {
        let input_idx = img_input_start + i;
        let next_label = format!("[base{}]", label_idx);
        label_idx += 1;
        let alpha_f = ov.alpha.clamp(0.0, 1.0);
        let img_label = format!("[img{}]", i);
        let scale_filter = match (ov.width, ov.height) {
            (Some(w), Some(h)) => format!("[{}:v]scale={}:{}:flags=lanczos,setpts=PTS-STARTPTS,format=rgba,colorchannelmixer=aa={:.4}{}", input_idx, w as u32, h as u32, alpha_f, img_label),
            (Some(w), None)    => format!("[{}:v]scale={}:-1:flags=lanczos,setpts=PTS-STARTPTS,format=rgba,colorchannelmixer=aa={:.4}{}", input_idx, w as u32, alpha_f, img_label),
            (None, Some(h))    => format!("[{}:v]scale=-1:{}:flags=lanczos,setpts=PTS-STARTPTS,format=rgba,colorchannelmixer=aa={:.4}{}", input_idx, h as u32, alpha_f, img_label),
            (None, None)       => format!("[{}:v]setpts=PTS-STARTPTS,format=rgba,colorchannelmixer=aa={:.4}{}", input_idx, alpha_f, img_label),
        };
        filter_parts.push(scale_filter);
        let img_overlay = format!("{}{}overlay={}:{}:format=auto{}", current_base, img_label, ov.x as i32, ov.y as i32, next_label);
        filter_parts.push(img_overlay);
        current_base = next_label;
    }

    let filter_string = if is_luma {
        match (args.overlay_width, args.overlay_height) {
            (Some(ow), Some(oh)) => format!(
                "[1:v]split=2[c][a]; [c]crop=w=iw/2:h=ih:x=0:y=0[color]; \
                 [a]crop=w=iw/2:h=ih:x=iw/2:y=0,format=gray[alpha]; \
                 [color][alpha]alphamerge[matte]; [matte]scale={}:{}[scaled_chat]; \
                 {}[scaled_chat]overlay={}:{}:shortest=1:{}[outv]",
                ow, oh, current_base, ox, oy, eof_action
            ),
            _ => format!(
                "[1:v]split=2[c][a]; [c]crop=w=iw/2:h=ih:x=0:y=0[color]; \
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
    let full_filter = filter_parts.join("; ");

    a.extend(["-filter_complex".into(), full_filter, "-map".into(), "[outv]".into(),
        "-map".into(), "0:a?".into(), "-c:a".into(), "copy".into()]);

    if has_nvenc {
        a.extend(["-c:v".into(), "h264_nvenc".into(), "-preset".into(), ffmpeg_preset.into(), "-cq".into(), "20".into()]);
    } else {
        a.extend(["-c:v".into(), "libx264".into(), "-preset".into(), ffmpeg_preset.into(),
            "-crf".into(), "20".into(), "-pix_fmt".into(), "yuv420p".into(),
            "-threads".into(), encode_threads.to_string()]);
    }
    a.push(args.output_path.clone());
    a
}

fn build_standalone_ffmpeg_args(
    args: &RenderVideoArgs,
    actual_width: i32,
    has_nvenc: bool,
    ffmpeg_preset: &str,
    encode_threads: usize,
) -> Vec<String> {
    let mut a = vec!["-y".to_string()];
    let (vcodec, pix_fmt) = match args.background_mode {
        BackgroundMode::Transparent => ("prores_ks", "yuva444p10le"),
        _ => if has_nvenc { ("h264_nvenc", "yuv420p") } else { ("libx264", "yuv420p") },
    };
    a.extend([
        "-thread_queue_size".into(),
        args.ffmpeg_input_queue_frames.unwrap_or(16).clamp(4, MAX_FFMPEG_RAW_QUEUE_FRAMES).to_string(),
        "-f".into(), "rawvideo".into(), "-pix_fmt".into(), "bgra".into(),
        "-s".into(), format!("{}x{}", actual_width, args.height),
        "-r".into(), args.fps.to_string(), "-i".into(), "-".into(),
        "-c:v".into(), vcodec.into(), "-pix_fmt".into(), pix_fmt.into(),
    ]);
    if vcodec == "prores_ks" {
        a.extend(["-profile:v".into(), "4444".into()]);
    } else {
        a.extend(["-preset".into(), ffmpeg_preset.into(),
            "-cq".into(), "20".into(), "-crf".into(), "20".into(),
            "-threads".into(), encode_threads.to_string()]);
    }
    a.push(args.output_path.clone());
    a
}

// ─────────────────────────────────────────────────────────────────────────────
// process_chat_render — main async entry point
//
// Pipeline overview:
//
//   [scan thread]
//       │  StreamDeserializer (line-by-line, 2 MiB BufReader — no full-file load)
//       │  → collects emote IDs / image URLs
//       │  → sends (MessageSaved, is_grouped) to loader_rx
//       │  → sends prefill candidates to prefill_rx
//       ↓
//   [layout thread]
//       │  Sliding window: up to 4 batches of 128 msgs in-flight on rayon
//       │  • Per-batch deduplication eliminates repeated layout work
//       │  • Per-thread LAYOUT_CACHE reuses baked TextBlobs across batches
//       │  → sends (spawn_frame, Arc<ScheduledMessage>) to stamp_rx
//       ↓
//   [frame render loop — main task thread]
//       │  • Frame-signature hash: skip render when output is identical
//       │  • Viewport culling: break when y_cursor < 0
//       │  • Chunk dispatch: N frames → rayon parallel render
//       │  • Pixel pool: zero heap allocation per frame
//       ↓
//   [IO writer thread]
//       │  Dedicated OS thread with 8 MiB BufWriter — never blocks rayon
//       ↓
//   [FFmpeg stdin] → encode → output file
// ─────────────────────────────────────────────────────────────────────────────

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

    if args.use_immediate_pipe_overlay {
        args.background_mode = BackgroundMode::Transparent;
    }

    // Fire both subprocess probes concurrently. Each takes ~50–150 ms;
    // running in parallel costs max(t_nvenc, t_probe) instead of the sum.
    let nvenc_probe = std::thread::spawn(probe_nvenc);
    let video_path_for_probe = args.overlay_video_path.clone();
    let fps_for_probe = args.fps;
    let video_frames_probe = std::thread::spawn(move || {
        video_path_for_probe.as_deref().and_then(|p| probe_video_frames(p, fps_for_probe))
    });

    let has_nvenc = nvenc_probe.join().unwrap_or(false);
    let is_luma = matches!(args.background_mode, BackgroundMode::LumaMatte);
    // LumaMatte doubles the canvas width: colour on the left, mask on the right.
    let actual_width = if is_luma { args.width * 2 } else { args.width };

    // ── Thread / chunk sizing ─────────────────────────────────────────────────
    let max_threads = args.max_render_threads;
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    // Leave 2 logical CPUs free for FFmpeg, the OS, and other desktop apps.
    let background_default = |cap: usize| cpus.saturating_sub(2).clamp(1, cap);

    let (mut worker_threads, ffmpeg_preset) = if args.use_immediate_pipe_overlay {
        (max_threads.unwrap_or_else(|| background_default(4)),
         if has_nvenc { "p1" } else { "ultrafast" })
    } else {
        match args.quality_preset {
            QualityPreset::Draft    => (max_threads.unwrap_or(1).max(1),
                                        if has_nvenc { "p1" } else { "ultrafast" }),
            QualityPreset::Standard => (max_threads.unwrap_or_else(|| background_default(6)),
                                        if has_nvenc { "p3" } else { "ultrafast" }),
            QualityPreset::High     => (max_threads.unwrap_or_else(|| background_default(8)),
                                        if has_nvenc { "p5" } else { "veryfast" }),
        }
    };

    // Clamp worker count by the RAM budget so 4K jobs don't allocate 16 surfaces.
    let frame_bytes = (actual_width as usize)
        .saturating_mul(args.height.max(1) as usize)
        .saturating_mul(4);
    let memory_budget_bytes = args.render_memory_budget_mb
                                  .unwrap_or(DEFAULT_PIXEL_POOL_BUDGET_MB).clamp(64, 4096) * 1024 * 1024;
    if max_threads.is_none() && frame_bytes > 0 {
        let by_memory = (memory_budget_bytes / frame_bytes / 2).clamp(1, worker_threads);
        worker_threads = worker_threads.min(by_memory.max(1));
    }

    let chunk_size = worker_threads.saturating_mul(2).clamp(CHUNK_SIZE_MIN, CHUNK_SIZE_MAX);
    let encode_threads = ffmpeg_encode_threads(cpus, worker_threads, has_nvenc);

    let render_pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(worker_threads)
            .thread_name(|i| format!("engine-worker-{}", i))
            .build()
            .map_err(|e| AppError::InternalError(format!("Failed to build render pool: {}", e)))?,
    );

    let ffmpeg_args = if args.overlay_video_path.is_some() {
        build_overlay_ffmpeg_args(&args, actual_width, is_luma, has_nvenc, ffmpeg_preset, encode_threads)
    } else {
        build_standalone_ffmpeg_args(&args, actual_width, has_nvenc, ffmpeg_preset, encode_threads)
    };

    // ── IO writer + FFmpeg spawn ──────────────────────────────────────────────
    //
    // Two modes:
    //
    //   A) Normal file-output: spawn FFmpeg, write raw BGRA to its stdin pipe.
    //
    //   B) Direct pipe (`use_immediate_pipe_overlay`): skip FFmpeg entirely.
    //      Write raw BGRA straight to stdout. The consumer (OBS, vMix, NDI)
    //      reads the rawvideo stream and handles its own encoding, eliminating
    //      the FFmpeg encode round-trip and its associated latency (~1–3 frames).
    //
    //      Consumer startup example:
    //        ffplay -f rawvideo -pix_fmt bgra -s WxH -r FPS -i pipe:0
    //
    // In both cases the hot path (rayon → channel → IO thread) is identical.

    let (io_tx, io_rx) = crossbeam_channel::bounded::<Arc<ReusableBuffer>>(IO_CHANNEL_DEPTH);

    // The IO thread owns either the FFmpeg child stdin or raw stdout.
    // It returns the FFmpeg ExitStatus on join (or a synthetic success for direct pipe).
    let (mut ffmpeg_child_opt, io_thread) = if args.use_immediate_pipe_overlay {
        // Direct pipe mode — no FFmpeg child.
        let t = std::thread::spawn(move || {
            let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, std::io::stdout());
            while let Ok(frame) = io_rx.recv() {
                if let Some(data) = &frame.data {
                    if writer.write_all(data).is_err() { break; }
                }
            }
            let _ = writer.flush();
            for _ in io_rx {}
        });
        (None::<std::process::Child>, t)
    } else {
        let mut child = hidden_command("ffmpeg")
            .args(&ffmpeg_args)
            .stdin(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| AppError::Ffmpeg(e.to_string()))?;
        let ff_stdin = child.stdin.take().unwrap();
        // Dedicated OS thread with an 8 MiB BufWriter.
        // Rayon workers never touch the pipe — zero contention on the hot path.
        let t = std::thread::spawn(move || {
            let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, ff_stdin);
            while let Ok(frame) = io_rx.recv() {
                if let Some(data) = &frame.data {
                    if writer.write_all(data).is_err() { break; }
                }
            }
            let _ = writer.flush();
            drop(writer);
            for _ in io_rx {} // drain so senders can unblock
        });
        (Some(child), t)
    };

    // ── Scan pass ─────────────────────────────────────────────────────────────
    // Streams the JSONL file line-by-line using a 2 MiB BufReader — never
    // loads the entire file into RAM. Collects emote IDs and image URLs,
    // then routes each message to either the prefill or main layout channel.
    let skip_users_set: FxHashSet<String> = args.skip_users.iter().cloned().collect();

    let (loader_tx, loader_rx) = crossbeam_channel::bounded::<(MessageSaved, bool)>(4096);
    let (prefill_tx, prefill_rx) = crossbeam_channel::bounded::<(MessageSaved, bool, u32)>(512);
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

    let (meta_tx, meta_rx) = tokio::sync::oneshot::channel::<(f64, Vec<i32>, Vec<String>)>();

    std::thread::spawn(move || {
        let f = match std::fs::File::open(&scan_path) {
            Ok(f) => f,
            Err(_) => return,
        };
        // 2 MiB read buffer — processes multi-MB log files without thrashing
        // the OS page cache. StreamDeserializer yields one MessageSaved at a
        // time, so peak RAM usage is O(single message) regardless of log size.
        let mut reader = std::io::BufReader::with_capacity(2 << 20, f);

        let mut max_offset_sec: f64 = 0.0;
        let mut emote_ids: FxHashSet<i32> = FxHashSet::default();
        let mut provider_image_urls: FxHashSet<String> = FxHashSet::default();
        let mut last_user = String::new();
        let mut last_time = -1i64;

        let flags = &scan_args.emote_providers;
        let map_opt = if !scan_emote_map.is_empty() && flags.any_name_provider_enabled() {
            Some((&scan_emote_map, flags))
        } else {
            None
        };

        let mut line = String::with_capacity(512);
        loop {
            line.clear();
            let read = match std::io::BufRead::read_line(&mut reader, &mut line) {
                Ok(n) => n,
                Err(_) => break,
            };
            if read == 0 { break; }
            if scan_cancel.load(Ordering::Relaxed) { break; }

            let msg: MessageSaved = match serde_json::from_str(line.trim_end_matches(['\r', '\n'])) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if scan_skip.contains(&msg.sender.username) { continue; }

            if let Some(start) = scan_args.start_ms {
                if (msg.created_at_secs as u64 * 1000) < start { continue; }
            }
            if let Some(end) = scan_args.end_ms {
                if (msg.created_at_secs as u64 * 1000) > end { break; }
            }

            let offset = msg.range_offset_secs as f64;
            if offset > max_offset_sec { max_offset_sec = offset; }

            // Collect asset references from this message's tokens.
            let mut collect_assets = |tok: &MessageToken| {
                match tok {
                    MessageToken::KickEmote { id } => {
                        if flags.kick { emote_ids.insert(*id); }
                    }
                    MessageToken::ProviderEmote(e) => { provider_image_urls.insert(e.url.to_string()); }
                    MessageToken::Text(_) => {}
                }
            };

            if do_prefill && offset < 0.0 {
                let age_secs = -offset;
                if age_secs <= prefill_window_secs as f64 {
                    let age_offset_frames = (age_secs * scan_args.fps as f64).round() as u32;
                    for tok in tokenise(&msg.content, map_opt) { collect_assets(&tok); }
                    let _ = prefill_tx.send((msg, false, age_offset_frames));
                }
                continue;
            }

            for tok in tokenise(&msg.content, map_opt) { collect_assets(&tok); }

            let is_grouped = group_enabled
                && msg.sender.username == last_user
                && (msg.range_offset_secs - last_time) <= group_window;
            last_user.clear();
            last_user.push_str(&msg.sender.username);
            last_time = msg.range_offset_secs;

            if loader_tx.send((msg, is_grouped)).is_err() { break; }
        }

        let _ = meta_tx.send((
            max_offset_sec,
            emote_ids.into_iter().collect(),
            provider_image_urls.into_iter().collect(),
        ));
    });

    let (max_offset_sec, emote_ids, provider_image_urls) =
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
        &args.emote_cache_policy,
    ));
    let img_cache = Arc::new(ImageCache::new(
        cache_dir_base.join("image_cache"),
        args.max_cached_emotes,
        target_emote_h * 4,
        args.quality_preset.clone(),
        args.eager_gif_decode,
        &args.emote_cache_policy,
    ));

    // Fetch both caches concurrently — network bound.
    let (emote_result, img_result) = tokio::join!(
        emote_cache.ensure_cached(&emote_ids),
        img_cache.ensure_cached(&provider_image_urls),
    );
    emote_result?;
    img_result?;

    // ── Font setup ────────────────────────────────────────────────────────────
    // FontMgr is !Send — scope it to a block so it is dropped before any
    // subsequent .await. Only Typeface (which IS Send) escapes the block.
    let (message_font, username_font, msg_line_h, metrics_ascent) = {
        let font_mgr = FontMgr::new();
        let typeface = font_mgr
            .match_family_style(&args.font_name, FontStyle::normal())
            .or_else(|| font_mgr.match_family_style("Apple Color Emoji", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("Segoe UI Emoji", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("Noto Color Emoji", FontStyle::normal()))
            .ok_or_else(|| AppError::InternalError("No system fonts found".into()))?;
        let mf = Font::from_typeface(typeface.clone(), args.font_size);
        let uf = Font::from_typeface(typeface, (args.font_size * 0.95).max(12.0));
        let (_, met) = mf.metrics();
        let lh = (met.descent - met.ascent) + args.line_spacing as f32;
        (mf, uf, lh, met.ascent)
    };

    // ── Visible bubble capacity ────────────────────────────────────────────────
    // Maximum number of bubbles that can reasonably fit in the viewport.
    // This is shared by the layout pipeline and the frame-render loop.
    let canvas_max_h = args.height - args.padding;

    let cull_min_bubble_h = (msg_line_h.ceil() as i32)
        .saturating_add(args.bubble_padding.max(0).saturating_mul(2))
        .saturating_add(args.message_spacing.max(0))
        .max(1);

    let guaranteed_visible_capacity =
        ((canvas_max_h.max(1) / cull_min_bubble_h) as usize)
            .saturating_add(3);

    // ── Layout thread ─────────────────────────────────────────────────────────
    //
    // Sliding-window design: keeps up to MAX_BATCHES_IN_FLIGHT batches of 128
    // messages submitted to rayon simultaneously. This keeps rayon workers and
    // the message-receive path busy concurrently, eliminating the staircase
    // pipeline stall of the previous stop-the-world design.
    //
    // Within each batch, identical messages are deduplicated before rayon sees
    // them: the key is (content, username, color, grouping) — a common spam
    // burst of 64 identical messages pays one layout cost, not 64.
    let args_pr = args.clone();
    let emote_cache_pr = Arc::clone(&emote_cache);
    let img_cache_pr = Arc::clone(&img_cache);
    let pr_cancel = Arc::clone(&cancel_flag);
    let render_pool_pr = Arc::clone(&render_pool);
    let highlight_set: FxHashSet<String> = args.pinned_users.iter().cloned().collect();
    let emote_map_pr = emote_map.clone();
    // Passed into the layout thread so submit_batch can drop invisible messages
    // before they ever reach rayon. Computed from canvas geometry above.
    let layout_max_per_frame = guaranteed_visible_capacity;

    std::thread::spawn(move || {
        // ── Prefill pass ──────────────────────────────────────────────────────
        if do_prefill {
            let mut prefill_msgs: Vec<(MessageSaved, bool, u32)> = prefill_rx.iter().collect();
            // Oldest-first: largest age_offset_frames first.
            prefill_msgs.sort_unstable_by(|a, b| b.2.cmp(&a.2));

            if !prefill_msgs.is_empty() {
                let (pfx, pfrx) = crossbeam_channel::bounded::<Vec<Option<ScheduledMessage>>>(1);
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
                                    let is_highlighted = hl.contains(&msg.sender.username);
                                    match layout_message_blocking(
                                        &msg.content, &msg.sender.username,
                                        &msg.sender.identity.color,
                                        &uf, &mf,
                                        (args_c.width - 2 * args_c.padding) as f32,
                                        mh, ma, &ec, &ic, &args_c, &em,
                                        &mut mc, 0, false,
                                    ) {
                                        Ok((lines, bw, bh, uc)) => {
                                            if lines.is_empty() || lines.iter().all(|l| l.tokens.is_empty()) {
                                                return None;
                                            }
                                            Some(ScheduledMessage::new_prefill(
                                                age_offset, lines, bw, bh,
                                                Color::from(&args_c.bubble_color), uc,
                                                false, is_highlighted,
                                                message_layout_key(&msg, false),
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

                if let Ok(results) = pfrx.recv() {
                    for sched in results.into_iter().flatten() {
                        let _ = stamp_tx.send((0, Arc::new(sched)));
                    }
                }
            }
        }

        // ── Sliding-window layout pipeline ────────────────────────────────────
        const BATCH_SIZE: usize = 128;
        const MAX_BATCHES_IN_FLIGHT: usize = 4;

        type BatchResult = Vec<Option<(i64, ScheduledMessage)>>;
        let mut in_flight: VecDeque<crossbeam_channel::Receiver<BatchResult>> =
            VecDeque::with_capacity(MAX_BATCHES_IN_FLIGHT);

        let mut batch: Vec<(MessageSaved, bool)> = Vec::with_capacity(BATCH_SIZE);
        let mut last_assigned_frame = -1i64;
        let mut layout_gen: u32 = 0;

        let submit_batch = |msgs: Vec<(MessageSaved, bool)>,
                            gen: u32,
                            pool: &rayon::ThreadPool,
                            max_per_frame: usize|
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
                // ── Per-frame capacity cap ────────────────────────────────────
                // Messages are ordered chronologically. Multiple messages can
                // share the same timestamp (same base_frame). If more arrive
                // for a given frame than can ever fit on screen, the excess are
                // dropped here — before dedup, before rayon, before any text
                // shaping. This is the only place where layout work is provably
                // skippable based on geometry alone.
                //
                // We keep the NEWEST `max_per_frame` messages per frame (the
                // ones at the tail of the timestamp group), because the render
                // loop displays newest-at-bottom — so the visible ones are the
                // last to arrive chronologically.
                let msgs = if msgs.len() > max_per_frame {
                    // Count how many messages share each base_frame.
                    // One pass: build (base_frame → count), then a second pass
                    // keeps only the last `max_per_frame` per frame.
                    let mut frame_counts: FxHashMap<i64, usize> =
                        FxHashMap::with_capacity_and_hasher(msgs.len(), Default::default());
                    for (msg, _) in &msgs {
                        let base = (msg.range_offset_secs as f64).max(0.0) as i64;
                        *frame_counts.entry(base).or_insert(0) += 1;
                    }
                    // For each frame, only keep the last max_per_frame.
                    let mut seen: FxHashMap<i64, usize> =
                        FxHashMap::with_capacity_and_hasher(frame_counts.len(), Default::default());
                    msgs.into_iter().filter(|(msg, _)| {
                        let base = (msg.range_offset_secs as f64).max(0.0) as i64;
                        let total = *frame_counts.get(&base).unwrap_or(&1);
                        let skip = total.saturating_sub(max_per_frame);
                        let idx = seen.entry(base).or_insert(0);
                        let keep = *idx >= skip;
                        *idx += 1;
                        keep
                    }).collect()
                } else {
                    msgs
                };

                // Deduplicate identical messages within the batch.
                // Spam bursts (64× same emote) pay one layout cost, not 64.
                let mut unique: Vec<(MessageSaved, bool)> = Vec::with_capacity(msgs.len());
                let mut key_to_unique: FxHashMap<u64, usize> =
                    FxHashMap::with_capacity_and_hasher(msgs.len(), Default::default());
                let mut remap: Vec<usize> = Vec::with_capacity(msgs.len());

                for (msg, is_grouped) in msgs {
                    let key = message_layout_key(&msg, is_grouped);
                    if let Some(&idx) = key_to_unique.get(&key) {
                        remap.push(idx);
                    } else {
                        let idx = unique.len();
                        key_to_unique.insert(key, idx);
                        unique.push((msg, is_grouped));
                        remap.push(idx);
                    }
                }

                let unique_results: Vec<Option<(i64, ScheduledMessage)>> = unique
                    .into_par_iter()
                    .map(|(msg, is_grouped)| {
                        PRE_RENDER_MEASURE_CACHE.with(|cc| {
                            MEASURE_GENERATION.with(|gc| {
                                let mut mc = cc.borrow_mut();
                                let mut tg = gc.borrow_mut();
                                *tg = gen;

                                let offset_sec = (msg.range_offset_secs as f64).max(0.0);
                                let base_frame = (offset_sec * args_c.fps as f64).round() as i64;
                                let is_highlighted = hl.contains(&msg.sender.username);
                                let layout_key = message_layout_key(&msg, is_grouped);

                                // Per-thread layout cache: check before calling
                                // layout_message_blocking (which involves text shaping).
                                let cached = LAYOUT_CACHE.with(|cell| cell.borrow().get(&layout_key).cloned());
                                let layout = if let Some(c) = cached {
                                    Ok(c)
                                } else {
                                    let result = layout_message_blocking(
                                        &msg.content, &msg.sender.username,
                                        &msg.sender.identity.color,
                                        &uf, &mf,
                                        (args_c.width - 2 * args_c.padding) as f32,
                                        mh, ma, &ec, &ic, &args_c, &em,
                                        &mut mc, gen, is_grouped,
                                    ).map(|(lines, width, height, user_color)| CachedLayout {
                                        lines, width, height, user_color, gen,
                                    });
                                    if let Ok(ref c) = result {
                                        if !c.lines.is_empty() && c.lines.iter().any(|l| !l.tokens.is_empty()) {
                                            LAYOUT_CACHE.with(|cell| {
                                                let mut cache = cell.borrow_mut();
                                                // Generational eviction: retain entries from the
                                                // current and previous generation instead of nuking
                                                // the entire cache. Hot emotes computed earlier in
                                                // the same batch survive; truly stale entries don't.
                                                if cache.len() >= 512 {
                                                    evict_old_layout_entries(&mut cache, gen);
                                                }
                                                cache.insert(layout_key, c.clone());
                                            });
                                        }
                                    }
                                    result
                                };

                                match layout {
                                    Ok(c) => {
                                        if c.lines.is_empty() || c.lines.iter().all(|l| l.tokens.is_empty()) {
                                            return None;
                                        }
                                        Some((base_frame, ScheduledMessage::new(
                                            0, c.lines, c.width, c.height,
                                            Color::from(&args_c.bubble_color),
                                            c.user_color, is_grouped, is_highlighted, layout_key,
                                        )))
                                    }
                                    Err(_) => None,
                                }
                            })
                        })
                    })
                    .collect();

                let results: BatchResult = remap.into_iter()
                                                .map(|idx| unique_results[idx].clone())
                                                .collect();
                let _ = tx.send(results);
            });
            rx
        };

        let drain_one = |rx: crossbeam_channel::Receiver<BatchResult>,
                         last_frame: &mut i64,
                         stamp: &crossbeam_channel::Sender<(u32, Arc<ScheduledMessage>)>|
            -> bool {
            let results = match rx.recv() { Ok(r) => r, Err(_) => return false };
            let mut cursor = *last_frame;
            for (base_frame, mut sched) in results.into_iter().flatten() {
                let assigned = base_frame.max(cursor);
                cursor = assigned;
                sched.spawn_frame = assigned as u32;
                if stamp.send((sched.spawn_frame, Arc::new(sched))).is_err() { return false; }
            }
            *last_frame = cursor;
            true
        };

        loop {
            if pr_cancel.load(Ordering::Relaxed) { break; }

            match loader_rx.try_recv() {
                Ok(msg_tuple) => {
                    batch.push(msg_tuple);
                    if batch.len() >= BATCH_SIZE {
                        if in_flight.len() >= MAX_BATCHES_IN_FLIGHT {
                            if let Some(rx) = in_flight.pop_front() {
                                if !drain_one(rx, &mut last_assigned_frame, &stamp_tx) { break; }
                            }
                        }
                        layout_gen = layout_gen.wrapping_add(1);
                        let rx = submit_batch(std::mem::take(&mut batch), layout_gen, &render_pool_pr, layout_max_per_frame);
                        in_flight.push_back(rx);
                        batch.reserve(BATCH_SIZE);
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    if let Some(rx) = in_flight.pop_front() {
                        if !drain_one(rx, &mut last_assigned_frame, &stamp_tx) { break; }
                    } else {
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

        if !batch.is_empty() {
            layout_gen = layout_gen.wrapping_add(1);
            let rx = submit_batch(batch, layout_gen, &render_pool_pr, layout_max_per_frame);
            in_flight.push_back(rx);
        }
        for rx in in_flight {
            if !drain_one(rx, &mut last_assigned_frame, &stamp_tx) { break; }
        }
    });

    // ── Frame render loop ─────────────────────────────────────────────────────
    let chat_total_frames = ((max_offset_sec * args.fps as f64).round() as u32)
        .saturating_add(args.message_hold_seconds * args.fps);

    let total_frames = {
        let probed = video_frames_probe.join().unwrap_or(None);
        probed.unwrap_or(chat_total_frames).min(chat_total_frames)
    };

    let bg_color = match args.background_mode {
        BackgroundMode::Transparent => Color::TRANSPARENT,
        BackgroundMode::LumaMatte => Color::BLACK,
        BackgroundMode::ChromaKeyGreen => Color::from_argb(255, 0, 255, 0),
        BackgroundMode::CustomColor => Color::from(&args.background_color),
    };

    // BGRA8888 + Premul: matches FFmpeg's rawvideo input format exactly.
    // No pixel-format conversion overhead on the CPU side.
    let info = ImageInfo::new(
        (actual_width, args.height),
        ColorType::BGRA8888,
        AlphaType::Premul,
        None,
    );
    let num_bytes = (actual_width * args.height * 4) as usize;

    let pixel_pool = Arc::new(PixelBufferPool::new(pixel_pool_buffer_count(
        num_bytes, worker_threads, args.render_memory_budget_mb,
    )));

    let mut active_bubbles: VecDeque<Arc<ScheduledMessage>> = VecDeque::new();
    let mut next_stamp: Option<(u32, Arc<ScheduledMessage>)> = None;
    // `bool` = dirty flag only — no owned Vec per frame.
    // The rayon block borrows `active_bubbles` directly as a contiguous slice.
    let mut frame_chunk: Vec<(u32, bool)> = Vec::with_capacity(chunk_size);

    let fps_f32 = args.fps as f32;
    let hold_secs = args.message_hold_seconds as f32;
    let fade_secs = args.message_fade_out_seconds as f32;
    let anim_slide = args.anim_slide;
    let anim_fade = args.anim_fade_in;
    let eviction = args.eviction_strategy.clone();

    let cull_cutoff     = args.height + args.padding;
    let cull_spacing    = args.message_spacing;
    // Scalar copy used inside the rayon closure — avoids cloning RenderVideoArgs.
    let canvas_height   = args.height;

    let mut last_sig: u64 = u64::MAX;
    let mut last_buf: Option<Arc<ReusableBuffer>> = None;

    // Pre-allocated per-chunk work vectors. Hoisted outside the frame loop so
    // the backing heap allocation survives chunk boundaries — only the *length*
    // is reset via `.clear()` on each chunk, never the capacity.
    // dirty_frame_ids: the frame IDs that need a new render this chunk.
    let mut dirty_frame_ids: Vec<u32> = Vec::with_capacity(chunk_size);
    let mut sequence: Vec<Result<usize, ()>> = Vec::with_capacity(chunk_size);

    // Constant draw parameters computed once for the entire render job.
    // Hoisted here so draw_frame never repeats Color::from / field access work.
    let draw_msg_color  = Color::from(&args.message_color);
    let draw_hi_color   = Color::from(&args.highlight_color);
    let draw_outline_w  = args.username_outline_width.unwrap_or(1.5);
    let draw_fade_out_f = args.message_fade_out_seconds as f32;
    let draw_y_start    = (args.height - args.padding) as f32;
    let draw_padding_f  = args.padding as f32;
    let draw_spacing_f  = args.message_spacing as f32;
    let draw_width_f    = args.width as f32;
    let draw_radius     = args.bubble_radius;
    let draw_username_shadow  = args.username_shadow;
    let draw_outline_usernames = args.outline_usernames;

    emit_progress(10.0, "Rendering frames...");
    let mut last_progress_emit = std::time::Instant::now();

    'frame: for f_idx in 0..total_frames {
        if cancel_flag.load(Ordering::Relaxed) { break; }

        // Drain newly spawned bubbles whose frame has arrived.
        // The capacity check happens here, not after, so excess bubbles from
        // a same-frame burst are skipped immediately without being pushed
        // into active_bubbles and then truncated — no Arc moves, no drops.
        //
        // guaranteed_visible_capacity is computed once per job above (cull constants).
        // Using it here means we never hold more than ~screen_height/min_bubble_h + 3
        // bubbles in active_bubbles at any point, even during a 500-msg/frame burst.
        loop {
            if next_stamp.is_none() { next_stamp = stamp_rx.try_recv().ok(); }
            match &next_stamp {
                Some((spawn_frame, _)) if *spawn_frame <= f_idx => {
                    let bubble = next_stamp.take().unwrap().1;
                    // Only push if there is still visible room. Newer messages
                    // go to the front (push_front); the deque fills front-to-back
                    // so capacity is measured against the guaranteed visible count.
                    if active_bubbles.len() < guaranteed_visible_capacity {
                        active_bubbles.push_front(bubble);
                    }
                    // else: drop the Arc here — zero truncate cost later.
                }
                _ => break,
            }
        }

        // Fine-grained height-based trim: remove bubbles that cannot be visible
        // given the actual cumulative height of bubbles above them.
        // The guaranteed_visible_capacity pre-check above means this retain
        // rarely needs to remove anything — it only fires for edge cases where
        // bubble heights are uneven enough that fewer fit than the minimum estimate.
        {
            let mut cum_h = 0i32;
            active_bubbles.retain(|b| {
                cum_h += b.bubble_h + cull_spacing;
                cum_h <= cull_cutoff + b.bubble_h
            });
        }

        // ── Frame signature ───────────────────────────────────────────────────
        // Skip the render entirely when the visual output hasn't changed.
        // The signature hash covers all visible state: bubble identity, alpha,
        // slide offset (bucketed), and animated-emote frame index.
        let sig = frame_signature_deque(
            &active_bubbles, f_idx, fps_f32, anim_slide, anim_fade, &eviction, hold_secs, fade_secs,
        );
        let dirty = sig != last_sig || last_buf.is_none();
        // Record only whether this frame needs a new render — no snapshot Vec.
        frame_chunk.push((f_idx, dirty));
        if dirty { last_sig = sig; }

        if frame_chunk.len() < chunk_size && f_idx < total_frames - 1 {
            continue;
        }

        // ── Build unique job list from chunk ──────────────────────────────────
        // Reuse the pre-allocated vecs; .clear() preserves capacity.
        dirty_frame_ids.clear();
        sequence.clear();

        for (frame_id, is_dirty) in &frame_chunk {
            if *is_dirty {
                let job_idx = dirty_frame_ids.len();
                dirty_frame_ids.push(*frame_id);
                sequence.push(Ok(job_idx));
            } else {
                sequence.push(Err(()));
            }
        }

        // ── Parallel render ───────────────────────────────────────────────────
        // `active_bubbles.make_contiguous()` is O(1) when the deque hasn't
        // wrapped (the common case). We borrow the resulting slice for the
        // entire duration of `render_pool.install()` — which is synchronous —
        // so no Arc ref-count churn and no per-frame Vec allocation.
        if !dirty_frame_ids.is_empty() {
            let bubbles_slice: &[Arc<ScheduledMessage>] =
                active_bubbles.make_contiguous();

            let pool = Arc::clone(&pixel_pool);
            let info_clone = info.clone();

            let rendered_jobs: Vec<Arc<ReusableBuffer>> = render_pool.install(|| {
                dirty_frame_ids.par_iter().copied().map(|frame_id| {
                    let mut buf = ReusableBuffer::new(pool.clone(), num_bytes);

                    SKIA_SURFACE.with(|surf_cell| {
                        let mut surf_opt = surf_cell.borrow_mut();

                        // Lazily initialise the per-thread Skia raster surface.
                        // `canvas_height` is a plain i32 copy — no args clone needed.
                        if surf_opt.is_none()
                            || surf_opt.as_ref().unwrap().width()  != actual_width
                            || surf_opt.as_ref().unwrap().height() != canvas_height
                        {
                            *surf_opt = Some(
                                surfaces::raster(&info_clone, None, None).unwrap(),
                            );
                        }

                        let surface = surf_opt.as_mut().unwrap();
                        let canvas = surface.canvas();

                        draw_frame(
                            canvas, bubbles_slice,
                            bg_color, is_luma,
                            frame_id, fps_f32, hold_secs, draw_fade_out_f,
                            anim_slide, anim_fade, &eviction,
                            draw_msg_color, draw_hi_color, draw_outline_w,
                            draw_y_start, draw_padding_f, draw_spacing_f,
                            draw_width_f, draw_radius,
                            draw_username_shadow, draw_outline_usernames,
                        );

                        // read_pixels writes directly into the pool buffer —
                        // no intermediate copy; stride = actual_width * 4.
                        surface.read_pixels(
                            &info_clone,
                            buf.data.as_mut().unwrap().as_mut_slice(),
                            (actual_width * 4) as usize,
                            (0, 0),
                        );
                    });

                    Arc::new(buf)
                }).collect()
            });

            // ── Dispatch to IO thread ─────────────────────────────────────────
            // For dirty frames: send the newly rendered buffer.
            // For repeat frames: clone the Arc (8 bytes) so the IO thread sees
            // a reference to the exact same pixel data — no pixel copy.
            let mut channel_closed = false;
            let mut render_iter = rendered_jobs.into_iter();

            for directive in &sequence {
                let buf_to_send = match directive {
                    Ok(_) => {
                        let buf = render_iter.next().unwrap();
                        last_buf = Some(Arc::clone(&buf));
                        buf
                    }
                    Err(()) => Arc::clone(last_buf.as_ref().unwrap()),
                };
                if io_tx.send(buf_to_send).is_err() {
                    channel_closed = true;
                    break;
                }
            }
            if channel_closed { break 'frame; }
        } else if let Some(ref buf) = last_buf {
            // Entire chunk was identical — blast the same Arc N times.
            let buf = Arc::clone(buf);
            for _ in &sequence {
                if io_tx.send(Arc::clone(&buf)).is_err() { break 'frame; }
            }
        }

        frame_chunk.clear();

        let pct = 10.0 + ((f_idx as f32 / total_frames.max(1) as f32) * 90.0);
        if last_progress_emit.elapsed() >= std::time::Duration::from_millis(150)
            || f_idx + 1 >= total_frames
        {
            emit_progress(pct, &format!("Rendering... ({:.1}%)", pct));
            last_progress_emit = std::time::Instant::now();
        }
    }

    // Signal the IO thread that no more frames are coming, then close stdin
    // so FFmpeg sees EOF on the rawvideo stream and can begin muxing.
    drop(io_tx);

    let cancelled = cancel_flag.load(Ordering::SeqCst);

    // For direct pipe mode there is no FFmpeg child to wait on.
    // Join the IO thread (flushes stdout), then return immediately.
    if args.use_immediate_pipe_overlay {
        let _ = tokio::task::spawn_blocking(move || { let _ = io_thread.join(); }).await;
        return if cancelled {
            emit_progress(100.0, "Render Cancelled");
            Err(AppError::InternalError("Cancelled by user".into()))
        } else {
            emit_progress(100.0, "Complete");
            Ok(())
        };
    }

    // FFmpeg file-output path — wait for the child process to finish encoding.
    let shutdown_result = tokio::task::spawn_blocking(move || {
        let mut child = ffmpeg_child_opt.expect("ffmpeg child must be Some in file-output mode");
        if cancelled { let _ = child.kill(); }
        let _ = io_thread.join();
        child.wait()
    }).await;

    if cancelled {
        emit_progress(100.0, "Render Cancelled");
        return Err(AppError::InternalError("Cancelled by user".into()));
    }

    match shutdown_result {
        Ok(Ok(status)) if status.success() => { emit_progress(100.0, "Complete"); Ok(()) }
        Ok(Ok(status)) => {
            emit_progress(100.0, "Encoding failed");
            Err(AppError::Ffmpeg(format!("FFmpeg exited with status {}", status)))
        }
        Ok(Err(e)) => {
            emit_progress(100.0, "Encoding failed");
            Err(AppError::Ffmpeg(format!("FFmpeg wait error: {}", e)))
        }
        Err(e) => {
            emit_progress(100.0, "Encoding failed");
            Err(AppError::InternalError(format!("spawn_blocking panicked: {}", e)))
        }
    }
}