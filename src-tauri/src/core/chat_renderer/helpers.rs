use crate::core::chat_renderer::args::QualityPreset;
use crate::core::chat_renderer::types::EmoteData;
use crate::error::AppError;
use crate::types::AppResult;
use image::imageops::FilterType;
use image::{self, AnimationDecoder, DynamicImage, GenericImageView};
use skia_safe::{images, Color, Data, Image};
use std::io::Cursor;
use std::sync::Arc;

pub fn skia_color_from_hex(hex: &str) -> Option<Color> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(Color::from_rgb(
        ((value >> 16) & 0xFF) as u8,
        ((value >> 8) & 0xFF) as u8,
        (value & 0xFF) as u8,
    ))
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
    let bytes_len = bytes.len();

    if bytes_len >= 8 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        "png".to_string()
    } else if bytes_len >= 3 && &bytes[0..3] == b"\xff\xd8\xff" {
        "jpg".to_string()
    } else if bytes_len >= 6 && (&bytes[0..6] == b"GIF87a" || &bytes[0..6] == b"GIF89a") {
        "gif".to_string()
    } else if bytes_len >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
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

/// Helper function to perform binary search on GIF frames based on timeline position
pub fn frame_at_ms(cum: &[u32], total_ms: u32, t_ms: u64) -> usize {
    if cum.is_empty() || total_ms == 0 {
        return 0;
    }
    let looped = (t_ms % total_ms as u64) as u32;
    cum.partition_point(|&c| c <= looped)
        .min(cum.len().saturating_sub(1))
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
                    (numer, denom) if denom != 0 => numer / denom,
                    (numer, _) => numer,
                };

                let dyn_img = DynamicImage::ImageRgba8(frame.into_buffer());
                let resized = resize_dynamic_image_preserve_aspect(dyn_img, target_h, filter);
                let rgba = resized.to_rgba8();
                let (w, h) = rgba.dimensions();

                if w == 0 || h == 0 {
                    eprintln!("Warning: Skipping zero-size GIF frame");
                    continue;
                }

                let row_bytes = (w * 4) as usize;
                let data = Data::new_copy(rgba.as_raw());

                if let Some(img) = images::raster_from_data(
                    &skia_safe::ImageInfo::new(
                        (w as i32, h as i32),
                        skia_safe::ColorType::RGBA8888,
                        skia_safe::AlphaType::Unpremul,
                        None,
                    ),
                    &data,
                    row_bytes,
                ) {
                    durations_ms.push(delay_ms);
                    skia_frames.push(img);
                } else {
                    eprintln!("Warning: Skia rejected valid RGBA buffer for GIF frame");
                    continue;
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

            let row_bytes = (w * 4) as usize;
            let data = Data::new_copy(rgba.as_raw());

            let img = images::raster_from_data(
                &skia_safe::ImageInfo::new(
                    (w as i32, h as i32),
                    skia_safe::ColorType::RGBA8888,
                    skia_safe::AlphaType::Unpremul,
                    None,
                ),
                &data,
                row_bytes,
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
