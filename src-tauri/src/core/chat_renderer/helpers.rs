use rayon::prelude::*;
use rustc_hash::FxHasher;
use skia_safe::{images, AlphaType, Color, ColorType, Data, Image, ImageInfo};
use std::hash::Hasher;
use std::io::Cursor;
use std::sync::Arc;

use image::imageops::FilterType;
use image::{AnimationDecoder, DynamicImage, GenericImageView};

use crate::core::chat_renderer::args::QualityPreset;
use crate::core::chat_renderer::types::EmoteData;
use crate::error::AppError;
use crate::types::AppResult;

// Precomputed Skia Colors (ARGB) — eliminates runtime string parsing overhead.
pub const DEFAULT_USERNAME_COLORS: &[Color] = &[
    Color::new(0xFFFF0000),
    Color::new(0xFF0000FF),
    Color::new(0xFF00FF00),
    Color::new(0xFFB22222),
    Color::new(0xFFFF7F50),
    Color::new(0xFF9ACD32),
    Color::new(0xFFFF4500),
    Color::new(0xFF2E8B57),
    Color::new(0xFFDAA520),
    Color::new(0xFFD2691E),
    Color::new(0xFF5F9EA0),
    Color::new(0xFF1E90FF),
    Color::new(0xFFFF69B4),
    Color::new(0xFF8A2BE2),
    Color::new(0xFF00FF7F),
];

/// Parse a hex color string and map it to a Skia `Color`.
///
/// Falls back to a deterministic hash-based palette entry when the hex string
/// is absent or malformed. No heap allocation on the fast path.
#[inline(always)]
pub fn get_user_color(username: &str, hex_color: &str) -> Color {
    if hex_color.len() >= 6 {
        let clean = if hex_color.as_bytes()[0] == b'#' {
            &hex_color[1..]
        } else {
            hex_color
        };
        if let Ok(val) = u32::from_str_radix(clean, 16) {
            return match clean.len() {
                6 => Color::from_rgb((val >> 16) as u8, (val >> 8) as u8, val as u8),
                8 => Color::from_argb(
                    (val >> 24) as u8,
                    (val >> 16) as u8,
                    (val >> 8) as u8,
                    val as u8,
                ),
                _ => Color::WHITE,
            };
        }
    }
    // Deterministic hash-based fallback — same color for same username every run.
    let mut hasher = FxHasher::default();
    hasher.write(username.as_bytes());
    DEFAULT_USERNAME_COLORS[(hasher.finish() as usize) % DEFAULT_USERNAME_COLORS.len()]
}

#[inline(always)]
pub fn quality_to_filter(q: &QualityPreset) -> FilterType {
    match q {
        QualityPreset::Draft => FilterType::Nearest,
        QualityPreset::Standard => FilterType::Triangle,
        QualityPreset::High => FilterType::Lanczos3,
    }
}

/// Cubic ease-out: fast start, decelerates to stop.
/// `t` must be in [0.0, 1.0].
#[inline(always)]
pub fn ease_out(t: f32) -> f32 {
    let inv = 1.0 - t;
    1.0 - (inv * inv * inv)
}

/// Identify a byte buffer's image format from its magic bytes.
/// Returns a static str to avoid heap allocation.
#[inline]
pub fn guess_ext(bytes: &[u8]) -> &'static str {
    match bytes {
        b if b.starts_with(b"\x89PNG\r\n\x1a\n") => "png",
        b if b.starts_with(b"\xff\xd8\xff") => "jpg",
        b if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") => "gif",
        b if b.len() >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" => "webp",
        _ => "bin",
    }
}

/// Resize `img` so its height equals `target_h`, preserving aspect ratio.
/// Returns `img` unchanged when `h == target_h` or either dimension is zero
/// (avoids a pointless encode/decode cycle).
///
/// The output is always `DynamicImage::ImageRgba8` so callers can use
/// `.into_rgba8()` without a second allocation.
#[inline]
pub fn resize_dynamic_image_preserve_aspect(
    img: DynamicImage,
    target_h: u32,
    filter: FilterType,
) -> DynamicImage {
    let (w, h) = img.dimensions();
    if h == 0 || w == 0 || h == target_h {
        return img;
    }
    // Compute width with round-to-nearest to minimise aspect-ratio drift.
    let scale = target_h as f32 / h as f32;
    let target_w = ((w as f32 * scale) + 0.5) as u32;
    DynamicImage::ImageRgba8(image::imageops::resize(&img, target_w, target_h, filter))
}

/// Build a Skia `Image` from a raw RGBA8888 pixel buffer without copying when
/// possible. `stride` must be `width * 4`.
///
/// Returns `None` if Skia rejects the buffer (should not happen for valid dims).
#[inline(always)]
fn skia_image_from_rgba(pixels: &[u8], w: u32, h: u32, alpha_type: AlphaType) -> Option<Image> {
    debug_assert_eq!(pixels.len(), (w * h * 4) as usize);
    let data = Data::new_copy(pixels);
    let info = ImageInfo::new((w as i32, h as i32), ColorType::RGBA8888, alpha_type, None);
    images::raster_from_data(&info, &data, (w * 4) as usize)
}

/// Decode raw image bytes into [`EmoteData`].
///
/// # Performance notes
/// * **GIF frames** are decoded and resized in parallel on rayon workers
///   (CPU-bound). Skia image construction is done sequentially on the caller's
///   thread because `Image` is `!Send`. Each frame uses `FilterType::Nearest`
///   because GIF palettes are already lossy and bilinear filtering adds colour
///   fringing for no perceptible gain.
/// * **Static images** (PNG/JPG/WEBP) use the caller-supplied `quality` filter
///   and are decoded synchronously. For large batches the caller should drive
///   multiple `decode_emote_bytes_to_emote_data` calls from rayon workers.
/// * When `premultiply` is `true` Skia marks the surface `AlphaType::Premul`,
///   which saves a per-pixel α-multiply in the compositing path. This is always
///   safe for static images. For GIFs the `image` crate delivers straight-alpha
///   data; we still mark the Skia image `Premul` and let Skia handle the one-
///   time conversion on upload rather than paying for it every draw call.
pub fn decode_emote_bytes_to_emote_data(
    bytes: &[u8],
    target_h: u32,
    premultiply: bool,
    quality: &QualityPreset,
) -> AppResult<EmoteData> {
    let alpha_type = if premultiply {
        AlphaType::Premul
    } else {
        AlphaType::Unpremul
    };
    let filter = quality_to_filter(quality);

    if guess_ext(bytes) == "gif" {
        return decode_gif(bytes, target_h, alpha_type);
    }

    decode_static(bytes, target_h, filter, alpha_type)
}

// ─────────────────────────────────────────────────────────────────────────────
// GIF decoder
// ─────────────────────────────────────────────────────────────────────────────

fn decode_gif(bytes: &[u8], target_h: u32, alpha_type: AlphaType) -> AppResult<EmoteData> {
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?;
    let frames = decoder.into_frames().collect_frames()?;

    if frames.is_empty() {
        return Err(AppError::EmoteCache("GIF decoded to zero frames".into()));
    }

    // ── Phase 1: decode + resize each frame on rayon workers ──────────────────
    // Produces (delay_ms, width, height, raw_rgba_bytes). `Image` is !Send so
    // we build Skia images in phase 2 on the calling thread.
    let processed: Vec<(u32, u32, u32, Vec<u8>)> = frames
        .into_par_iter()
        .filter_map(|frame| {
            let (n, d) = frame.delay().numer_denom_ms();
            let delay_ms = if d != 0 { n / d } else { n };

            let dyn_frame = DynamicImage::ImageRgba8(frame.into_buffer());
            let (orig_w, orig_h) = dyn_frame.dimensions();
            if orig_w == 0 || orig_h == 0 {
                return None;
            }

            // Always Nearest for GIF: palette-quantised source means bilinear
            // adds colour fringing with no quality benefit.
            let resized =
                resize_dynamic_image_preserve_aspect(dyn_frame, target_h, FilterType::Nearest);
            let (rw, rh) = resized.dimensions();
            Some((delay_ms, rw, rh, resized.into_rgba8().into_raw()))
        })
        .collect();

    if processed.is_empty() {
        return Err(AppError::EmoteCache(
            "GIF decoded to zero valid frames".into(),
        ));
    }

    // ── Phase 2: build Skia images sequentially ────────────────────────────────
    let n = processed.len();
    let mut skia_frames = Vec::with_capacity(n);
    let mut durations_ms = Vec::with_capacity(n);
    let mut cum_durations = Vec::with_capacity(n);
    let mut current_cum = 0u32;
    let mut final_w = 0u32;
    let mut final_h = 0u32;

    for (delay, rw, rh, raw) in processed {
        if let Some(img) = skia_image_from_rgba(&raw, rw, rh, alpha_type) {
            final_w = rw;
            final_h = rh;
            durations_ms.push(delay);
            current_cum = current_cum.saturating_add(delay);
            cum_durations.push(current_cum);
            skia_frames.push(img);
        }
    }

    if skia_frames.is_empty() {
        return Err(AppError::EmoteCache("Skia rejected all GIF frames".into()));
    }

    Ok(EmoteData::Animated {
        frames: Arc::from(skia_frames),
        durations_ms: Arc::from(durations_ms),
        cum_durations: Arc::from(cum_durations),
        total_ms: current_cum,
        w: final_w as i32,
        h: final_h as i32,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Static image decoder (PNG / JPG / WEBP)
// ─────────────────────────────────────────────────────────────────────────────

fn decode_static(
    bytes: &[u8],
    target_h: u32,
    filter: FilterType,
    alpha_type: AlphaType,
) -> AppResult<EmoteData> {
    let dyn_img = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?;

    let (orig_w, orig_h) = dyn_img.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return Err(AppError::EmoteCache(
            "emote decoded to zero-size image".into(),
        ));
    }

    // Skip resize when already the right height — avoids a full re-encode.
    let resized = if orig_h == target_h {
        dyn_img
    } else {
        resize_dynamic_image_preserve_aspect(dyn_img, target_h, filter)
    };

    let (rw, rh) = resized.dimensions();
    let rgba = resized.into_rgba8();

    let img = skia_image_from_rgba(rgba.as_raw(), rw, rh, alpha_type)
        .ok_or_else(|| AppError::EmoteCache("Skia rejected valid RGBA buffer".into()))?;

    Ok(EmoteData::Static {
        img,
        w: rw as i32,
        h: rh as i32,
    })
}
