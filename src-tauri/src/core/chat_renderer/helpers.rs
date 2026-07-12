use crate::core::chat_renderer::args::QualityPreset;
use crate::core::chat_renderer::types::EmoteData;
use crate::error::AppError;
use crate::types::AppResult;
use image::imageops::FilterType;
use image::{self, AnimationDecoder, DynamicImage, GenericImageView};
use rustc_hash::FxHasher;
use skia_safe::{images, Color, Data, Image};
use std::hash::Hasher;
use std::io::Cursor;
use std::sync::Arc;

pub const DEFAULT_USERNAME_COLORS: &[&str] = &[
    "#FF0000", "#0000FF", "#00FF00", "#B22222", "#FF7F50", "#9ACD32", "#FF4500", "#2E8B57",
    "#DAA520", "#D2691E", "#5F9EA0", "#1E90FF", "#FF69B4", "#8A2BE2", "#00FF7F",
];

pub fn get_user_color(username: &str, hex_color: &str) -> Color {
    // Attempt to parse provided hex
    if !hex_color.is_empty() {
        let clean = hex_color.trim_start_matches('#');
        if let Ok(val) = u32::from_str_radix(clean, 16) {
            let color = if clean.len() == 6 {
                Color::from_rgb((val >> 16) as u8, (val >> 8) as u8, val as u8)
            } else if clean.len() == 8 {
                Color::from_argb(
                    (val >> 24) as u8,
                    (val >> 16) as u8,
                    (val >> 8) as u8,
                    val as u8,
                )
            } else {
                Color::WHITE
            };
            return color;
        }
    }

    // Fallback to deterministic C# Palette hash
    let mut hasher = FxHasher::default();
    hasher.write(username.as_bytes());
    let hash = hasher.finish();
    let hex = DEFAULT_USERNAME_COLORS[(hash as usize) % DEFAULT_USERNAME_COLORS.len()];

    let clean = hex.trim_start_matches('#');
    let val = u32::from_str_radix(clean, 16).unwrap_or(0xFFFFFF);
    Color::from_rgb((val >> 16) as u8, (val >> 8) as u8, val as u8)
}

pub fn quality_to_filter(q: &QualityPreset) -> FilterType {
    match q {
        QualityPreset::Draft => FilterType::Nearest,
        QualityPreset::Standard => FilterType::Triangle,
        QualityPreset::High => FilterType::Lanczos3,
    }
}

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

pub fn guess_ext(bytes: &[u8]) -> String {
    let len = bytes.len();
    if len >= 8 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        "png".to_string()
    } else if len >= 3 && &bytes[0..3] == b"\xff\xd8\xff" {
        "jpg".to_string()
    } else if len >= 6 && (&bytes[0..6] == b"GIF87a" || &bytes[0..6] == b"GIF89a") {
        "gif".to_string()
    } else if len >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp".to_string()
    } else {
        "bin".to_string()
    }
}

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
    let target_w = (w as f32 * scale).round() as u32;
    DynamicImage::ImageRgba8(image::imageops::resize(&img, target_w, target_h, filter))
}

pub fn decode_emote_bytes_to_emote_data(
    bytes: &[u8],
    target_h: u32,
    quality: &QualityPreset,
) -> AppResult<EmoteData> {
    let filter = quality_to_filter(quality);

    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Gif) => {
            let cur = Cursor::new(bytes);
            let decoder = image::codecs::gif::GifDecoder::new(cur)?;
            let frames_iter = decoder.into_frames();
            let frames_collected = frames_iter.collect_frames()?;

            let mut skia_frames: Vec<Image> = Vec::with_capacity(frames_collected.len());
            let mut durations_ms: Vec<u32> = Vec::with_capacity(frames_collected.len());

            for frame in frames_collected {
                let delay_ms = match frame.delay().numer_denom_ms() {
                    (n, d) if d != 0 => n / d,
                    (n, _) => n,
                };

                let dyn_img = DynamicImage::ImageRgba8(frame.into_buffer());
                let resized = resize_dynamic_image_preserve_aspect(dyn_img, target_h, filter);
                let rgba = resized.to_rgba8();
                let (w, h) = rgba.dimensions();

                if w == 0 || h == 0 {
                    continue;
                }

                let data = Data::new_copy(rgba.as_raw());
                if let Some(img) = images::raster_from_data(
                    &skia_safe::ImageInfo::new(
                        (w as i32, h as i32),
                        skia_safe::ColorType::RGBA8888,
                        skia_safe::AlphaType::Unpremul,
                        None,
                    ),
                    &data,
                    (w * 4) as usize,
                ) {
                    durations_ms.push(delay_ms);
                    skia_frames.push(img);
                }
            }

            let total_ms: u32 = durations_ms.iter().sum();
            let cum: Vec<u32> = durations_ms
                .iter()
                .scan(0u32, |acc, &d| {
                    *acc += d;
                    Some(*acc)
                })
                .collect();
            let (w, h) = skia_frames
                .first()
                .map(|f| (f.width(), f.height()))
                .unwrap_or((0, 0));

            Ok(EmoteData::Animated {
                frames: Arc::new(skia_frames),
                durations_ms: Arc::new(durations_ms),
                cum_durations: Arc::new(cum),
                total_ms,
                w,
                h,
            })
        }
        _ => {
            let dyn_img = image::ImageReader::new(Cursor::new(bytes))
                .with_guessed_format()?
                .decode()?;
            let resized = resize_dynamic_image_preserve_aspect(dyn_img, target_h, filter);
            let rgba = resized.to_rgba8();
            let (w, h) = rgba.dimensions();

            if w == 0 || h == 0 {
                return Err(AppError::EmoteCache(
                    "emote decoded to zero-size image".into(),
                ));
            }

            let data = Data::new_copy(rgba.as_raw());
            let img = images::raster_from_data(
                &skia_safe::ImageInfo::new(
                    (w as i32, h as i32),
                    skia_safe::ColorType::RGBA8888,
                    skia_safe::AlphaType::Unpremul,
                    None,
                ),
                &data,
                (w * 4) as usize,
            )
            .ok_or_else(|| AppError::EmoteCache("Skia rejected valid RGBA buffer".into()))?;

            Ok(EmoteData::Static {
                img,
                w: w as i32,
                h: h as i32,
            })
        }
    }
}
