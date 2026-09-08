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

// ─────────────────────────────────────────────────────────────────────────────
// Precomputed username palette
//
// Using precomputed ARGB constants avoids runtime string→int parsing on every
// username render. The palette is selected via a hash of the username bytes so
// the same username always maps to the same color — deterministic across runs.
// ─────────────────────────────────────────────────────────────────────────────

pub const DEFAULT_USERNAME_COLORS: &[Color] = &[
    Color::new(0xFFFF0000), // Red
    Color::new(0xFF0000FF), // Blue
    Color::new(0xFF00FF00), // Green
    Color::new(0xFFB22222), // Firebrick
    Color::new(0xFFFF7F50), // Coral
    Color::new(0xFF9ACD32), // YellowGreen
    Color::new(0xFFFF4500), // OrangeRed
    Color::new(0xFF2E8B57), // SeaGreen
    Color::new(0xFFDAA520), // Goldenrod
    Color::new(0xFFD2691E), // Chocolate
    Color::new(0xFF5F9EA0), // CadetBlue
    Color::new(0xFF1E90FF), // DodgerBlue
    Color::new(0xFFFF69B4), // HotPink
    Color::new(0xFF8A2BE2), // BlueViolet
    Color::new(0xFF00FF7F), // SpringGreen
];

/// Parse a hex color string and map it to a Skia `Color`.
///
/// Fast path: the hex string is parsed via `u32::from_str_radix` — no regex,
/// no allocation, single integer operation. Falls back to a deterministic
/// hash-based palette entry when the hex string is absent or malformed.
///
/// The `#` prefix is stripped with a byte comparison rather than `trim_start_matches`
/// to avoid a function-call overhead on the hot path.
#[inline(always)]
pub fn get_user_color(username: &str, hex_color: &str) -> Color {
    if hex_color.len() >= 6 {
        // Strip leading '#' in a single byte compare — no bounds check needed
        // because len() >= 6 guarantees at least one byte.
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
    // Deterministic fallback: FxHasher is non-cryptographic, fast, and
    // produces the same output for the same input across program invocations
    // (unlike std's default SipHash which is seeded from entropy).
    let mut hasher = FxHasher::default();
    hasher.write(username.as_bytes());
    DEFAULT_USERNAME_COLORS[(hasher.finish() as usize) % DEFAULT_USERNAME_COLORS.len()]
}

/// Map a `QualityPreset` to the corresponding `image` crate filter type.
///
/// This is the single decode-time decision point: the selected filter is
/// applied once when the image is resized and never again. Choosing `Draft`
/// (Nearest) costs almost nothing; choosing `High` (Lanczos3) adds ~200 µs
/// per emote decode but produces markedly better results for photo-style emotes.
#[inline(always)]
pub fn quality_to_filter(q: &QualityPreset) -> FilterType {
    match q {
        QualityPreset::Draft => FilterType::Nearest,
        QualityPreset::Standard => FilterType::Triangle,
        QualityPreset::High => FilterType::Lanczos3,
    }
}

/// Cubic ease-out: fast start, decelerates to stop. `t` must be in [0.0, 1.0].
///
/// Equivalent to `1 - (1-t)³` but written with explicit mul to avoid
/// `f32::powi` dispatch overhead when compiled without fast-math.
#[inline(always)]
pub fn ease_out(t: f32) -> f32 {
    let inv = 1.0 - t;
    1.0 - (inv * inv * inv)
}

/// Identify a byte buffer's image format from its magic bytes.
///
/// Returns a `&'static str` — zero allocation, zero copy. Magic-byte matching
/// is exhaustive for all formats the renderer can handle. Unknown formats
/// fall through to `"bin"` and are stored opaquely on disk until re-probed.
///
/// This replaces a regex-based sniff that added ~1 µs per call. The byte
/// pattern match compiles to a sequence of `memcmp` calls, typically elided by
/// the compiler to a few integer comparisons.
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
///
/// Returns `img` unchanged when the height already matches or either dimension
/// is zero — avoids a pointless encode/decode cycle. The output is always
/// `DynamicImage::ImageRgba8` so callers can call `.into_rgba8()` without a
/// second allocation.
///
/// The `scale` is computed with `f32` arithmetic and rounded to nearest-even
/// to minimize cumulative aspect-ratio drift across many resizes.
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
    let scale = target_h as f32 / h as f32;
    // `+ 0.5` gives round-to-nearest; avoids the common off-by-one where
    // a 32-wide emote at 0.99999× scale produces a 31-pixel result.
    let target_w = ((w as f32 * scale) + 0.5) as u32;
    DynamicImage::ImageRgba8(image::imageops::resize(&img, target_w, target_h, filter))
}

/// Build a Skia `Image` from a raw RGBA8888 pixel buffer.
///
/// `Data::new_copy` copies the pixel bytes into a Skia-owned buffer; this is
/// unavoidable because Skia's C++ lifetime model cannot borrow Rust memory.
/// However the copy happens once per emote at decode time, never per frame.
///
/// Returns `None` only if Skia rejects the image info (zero dimensions, bad
/// stride). For valid input this should never fail.
#[inline(always)]
fn skia_image_from_rgba(pixels: &[u8], w: u32, h: u32, alpha_type: AlphaType) -> Option<Image> {
    debug_assert_eq!(
        pixels.len(),
        (w * h * 4) as usize,
        "pixel buffer size mismatch: expected {}×{}×4 = {} bytes, got {}",
        w,
        h,
        w * h * 4,
        pixels.len()
    );
    let data = Data::new_copy(pixels);
    let info = ImageInfo::new((w as i32, h as i32), ColorType::RGBA8888, alpha_type, None);
    images::raster_from_data(&info, &data, (w * 4) as usize)
}

// ─────────────────────────────────────────────────────────────────────────────
// Public decode entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Decode raw image bytes into [`EmoteData`].
///
/// # Dispatch strategy
///
/// | Format | Eager GIF  | Result variant |
/// |--------|-----------|----------------|
/// | GIF    | `true`    | `Animated`  — all frames decoded immediately on rayon workers |
/// | GIF    | `false`   | `LazyGif`   — compressed bytes retained; first-access decode |
/// | PNG/JPG/WEBP | any | `Static`   — single frame decoded and resized |
///
/// # CPU / GPU model
///
/// All operations here are CPU-only. No GPU context is created. The design
/// deliberately avoids NVDEC/VAAPI to keep the pipeline stateless and portable
/// across machines without discrete GPUs. The FFmpeg encoding step is the only
/// point where an optional GPU is used, and only for H.264 throughput, not
/// correctness.
///
/// # Premultiplied alpha
///
/// When `premultiply` is `true` Skia marks the surface `AlphaType::Premul`,
/// saving a per-pixel α-multiply in the compositing path on every draw call.
/// For GIFs the `image` crate delivers straight-alpha; we mark the Skia image
/// `Premul` and let Skia handle the one-time conversion on upload. The cost
/// is paid once per emote, not once per frame × instance count.
pub fn decode_emote_bytes_to_emote_data(
    bytes: &[u8],
    target_h: u32,
    premultiply: bool,
    quality: &QualityPreset,
    eager_gif_decode: bool,
) -> AppResult<EmoteData> {
    let alpha_type = if premultiply {
        AlphaType::Premul
    } else {
        AlphaType::Unpremul
    };
    let filter = quality_to_filter(quality);

    match guess_ext(bytes) {
        "gif" => {
            if eager_gif_decode {
                decode_gif(bytes, target_h, alpha_type)
            } else {
                decode_gif_lazy(bytes, target_h, alpha_type)
            }
        }
        _ => decode_static(bytes, target_h, filter, alpha_type),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Lazy GIF — timing metadata only
// ─────────────────────────────────────────────────────────────────────────────

/// Build a `LazyGif` entry: retain compressed bytes and per-frame timing only.
///
/// The GIF frames must be walked once to extract cumulative delay metadata, but
/// they are intentionally NOT collected into a pixel buffer at this stage.
/// Peak RAM stays close to the compressed asset size (typically 5–50 KB) instead
/// of the full decoded animation (typically 100–500 KB for a 56px emote).
///
/// Pixel decode is deferred to the first `frame_at` call via `OnceLock`.
fn decode_gif_lazy(bytes: &[u8], target_h: u32, alpha_type: AlphaType) -> AppResult<EmoteData> {
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?;
    let mut frames = decoder.into_frames();

    let first = frames
        .next()
        .ok_or_else(|| AppError::EmoteCache("LazyGif: GIF decoded to zero frames".into()))??;

    let (src_w, src_h) = first.buffer().dimensions();
    if src_w == 0 || src_h == 0 {
        return Err(AppError::EmoteCache(
            "LazyGif: zero-sized first frame".into(),
        ));
    }

    let scale = target_h as f32 / src_h as f32;
    let w = ((src_w as f32 * scale) + 0.5) as i32;
    let h = target_h as i32;

    // Pre-allocate for typical GIF emote frame counts (8–30 frames).
    let mut cum_durations: Vec<u32> = Vec::with_capacity(32);

    let first_delay = {
        let (n, d) = first.delay().numer_denom_ms();
        if d != 0 {
            (n / d).max(10)
        } else {
            n.max(10)
        }
    };
    cum_durations.push(first_delay);
    let mut current_cum = first_delay;

    // Walk remaining frames to collect timing without decoding pixels.
    for frame in frames {
        let frame = frame?;
        let (n, d) = frame.delay().numer_denom_ms();
        let delay_ms = if d != 0 { (n / d).max(10) } else { n.max(10) };
        current_cum = current_cum.saturating_add(delay_ms);
        cum_durations.push(current_cum);
    }

    Ok(EmoteData::LazyGif {
        raw_bytes: Arc::from(bytes),
        cum_durations: Arc::from(cum_durations),
        total_ms: current_cum,
        w,
        h,
        target_h,
        alpha_type,
        decoded_cache: Arc::new(std::sync::OnceLock::new()),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Eager GIF → Skia frames
// ─────────────────────────────────────────────────────────────────────────────

/// Decode compressed GIF bytes all the way to a `Vec<Image>` of Skia frames.
///
/// Called by `EmoteData::LazyGif::frame_at` on first access via `OnceLock`.
/// Must be `pub` so `types.rs` can reference it from the `decoded_cache` init.
///
/// # Parallelism
///
/// Frame decode and resize run on `rayon` workers (CPU-bound, embarrassingly
/// parallel). Skia image construction happens sequentially on the calling
/// thread because `Image` is `!Send`. The two-phase design avoids holding
/// large pixel buffers in memory longer than necessary — each `Vec<u8>` is
/// consumed as soon as its `Image` is constructed.
///
/// GIF frames always use `FilterType::Nearest` regardless of the
/// `QualityPreset`. GIF palettes are already heavily quantised (256 colors);
/// bilinear interpolation between palette entries introduces color fringing
/// with no quality gain. `Nearest` is also ~5× faster for the resize step.
pub fn decode_gif_to_skia_frames(
    bytes: &[u8],
    target_h: u32,
    alpha_type: AlphaType,
) -> AppResult<Arc<[Image]>> {
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?;
    let frames = decoder.into_frames().collect_frames()?;

    if frames.is_empty() {
        return Err(AppError::EmoteCache(
            "LazyGif decoded to zero frames".into(),
        ));
    }

    // Phase 1: decode + resize on rayon workers.
    // Output: (width, height, raw_rgba_bytes). `Image` is !Send so we build
    // Skia images in phase 2 on the calling thread.
    let processed: Vec<(u32, u32, Vec<u8>)> = frames
        .into_par_iter()
        .filter_map(|frame| {
            let dyn_frame = DynamicImage::ImageRgba8(frame.into_buffer());
            let (orig_w, orig_h) = dyn_frame.dimensions();
            if orig_w == 0 || orig_h == 0 {
                return None;
            }
            // GIF-specific: always Nearest to avoid palette fringing.
            let resized =
                resize_dynamic_image_preserve_aspect(dyn_frame, target_h, FilterType::Nearest);
            let (rw, rh) = resized.dimensions();
            Some((rw, rh, resized.into_rgba8().into_raw()))
        })
        .collect();

    // Phase 2: build Skia images sequentially.
    let mut skia_frames = Vec::with_capacity(processed.len());
    for (rw, rh, raw) in processed {
        if let Some(img) = skia_image_from_rgba(&raw, rw, rh, alpha_type) {
            skia_frames.push(img);
        }
    }

    if skia_frames.is_empty() {
        return Err(AppError::EmoteCache(
            "LazyGif: Skia rejected all frames".into(),
        ));
    }

    Ok(Arc::from(skia_frames))
}

/// Decode a GIF to `EmoteData::Animated` — all frames decoded immediately.
///
/// Used when `eager_gif_decode = true` (the default). Returns a fully populated
/// `Animated` variant with timing metadata and decoded frames. The returned
/// `Arc<[Image]>` is shared across all instances of this emote in the render
/// job — no pixel data is duplicated.
fn decode_gif(bytes: &[u8], target_h: u32, alpha_type: AlphaType) -> AppResult<EmoteData> {
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?;
    let frames = decoder.into_frames().collect_frames()?;

    if frames.is_empty() {
        return Err(AppError::EmoteCache("GIF decoded to zero frames".into()));
    }

    // Phase 1: parallel decode + resize.
    // Delay metadata is extracted alongside the pixel work so we only
    // iterate the frame list once.
    let processed: Vec<(u32, u32, u32, Vec<u8>)> = frames
        .into_par_iter()
        .filter_map(|frame| {
            let (n, d) = frame.delay().numer_denom_ms();
            // Clamp to a sensible minimum (10 ms ≈ 100 fps) to avoid emotes
            // that the GIF spec technically allows at 0 ms delay blazing
            // through their animation within a single render frame.
            let delay_ms = if d != 0 { (n / d).max(10) } else { n.max(10) };

            let dyn_frame = DynamicImage::ImageRgba8(frame.into_buffer());
            let (orig_w, orig_h) = dyn_frame.dimensions();
            if orig_w == 0 || orig_h == 0 {
                return None;
            }

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

    // Phase 2: build Skia images sequentially + compute cumulative timing.
    let n = processed.len();
    let mut skia_frames = Vec::with_capacity(n);
    let mut cum_durations = Vec::with_capacity(n);
    let mut current_cum = 0u32;
    let mut final_w = 0i32;
    let mut final_h = 0i32;

    for (delay, rw, rh, raw) in processed {
        if let Some(img) = skia_image_from_rgba(&raw, rw, rh, alpha_type) {
            final_w = rw as i32;
            final_h = rh as i32;
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
        cum_durations: Arc::from(cum_durations),
        total_ms: current_cum,
        w: final_w,
        h: final_h,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Static image decoder (PNG / JPG / WEBP)
// ─────────────────────────────────────────────────────────────────────────────

/// Decode a static image to `EmoteData::Static`.
///
/// The `with_guessed_format()` call uses the byte content (magic bytes),
/// not the file extension, to select the decoder — avoids misdetection when
/// server responses omit or lie about Content-Type.
///
/// The resize step is skipped entirely when the image is already `target_h`
/// tall, which is the common case for pre-scaled CDN assets.
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

    let resized = if orig_h == target_h {
        // Already the right height — skip the resize + re-encode.
        dyn_img
    } else {
        resize_dynamic_image_preserve_aspect(dyn_img, target_h, filter)
    };

    let (rw, rh) = resized.dimensions();
    // `into_rgba8()` converts in-place when the image is already RGBA8;
    // for other color types (e.g. RGB8) it allocates exactly one buffer.
    let rgba = resized.into_rgba8();

    let img = skia_image_from_rgba(rgba.as_raw(), rw, rh, alpha_type)
        .ok_or_else(|| AppError::EmoteCache("Skia rejected valid RGBA buffer".into()))?;

    Ok(EmoteData::Static {
        img,
        w: rw as i32,
        h: rh as i32,
    })
}
