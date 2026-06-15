use crate::core::chat_renderer::types::EmoteData;
use crate::error::AppError;
use crate::types::AppResult;
use image::imageops::FilterType;
use image::ImageFormat::Png;
use image::{self, AnimationDecoder, DynamicImage, GenericImageView};
use skia_safe::{images, Color, Data, Image};
use std::io::Cursor;

/// Convert standard hex color (e.g. "#FF0000") -> skia Color
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

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

// Perhaps unnecesasry if the url only returns pngs and gifs
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

/// Resize DynamicImage preserving aspect to target height
pub fn resize_dynamic_image_preserve_aspect(img: DynamicImage, target_h: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if h == 0 || w == 0 {
        return img;
    }
    if h == target_h {
        return img;
    }

    let scale = (target_h as f32) / (h as f32);
    let target_w = ((w as f32) * scale).round() as u32;

    let resized: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
        image::imageops::resize(&img, target_w, target_h, FilterType::Lanczos3);

    DynamicImage::ImageRgba8(resized)
}

/// Decode raw bytes into EmoteData (static or animated GIF), resizing to target_h
/// Decode raw bytes into EmoteData (static or animated GIF), resizing to target_h
pub fn decode_emote_bytes_to_emote_data(bytes: &[u8], target_h: u32) -> AppResult<EmoteData> {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Gif) => {
            let cur = Cursor::new(bytes.to_vec());
            let decoder = image::codecs::gif::GifDecoder::new(cur)?;
            let frames_iter = decoder.into_frames();
            let frames_collected = frames_iter.collect_frames()?;

            let mut skia_frames: Vec<Image> = Vec::with_capacity(frames_collected.len());
            let mut durations_ms: Vec<u32> = Vec::with_capacity(frames_collected.len());

            for frame in frames_collected {
                let buffer = frame.buffer().clone();
                let dyn_img = DynamicImage::ImageRgba8(buffer);

                let resized = resize_dynamic_image_preserve_aspect(dyn_img, target_h);
                let rgba = resized.to_rgba8();
                let (w, h) = rgba.dimensions();
                let row_bytes = (w * 4) as usize;
                let data = Data::new_copy(rgba.as_raw());

                // FIXED: Using AlphaType::Unpremul prevents black backgrounds on transparent emotes
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
                    let delay_ms = match frame.delay().numer_denom_ms() {
                        (numer, denom) if denom != 0 => numer / denom,
                        (numer, _) => numer,
                    };
                    durations_ms.push(delay_ms);
                    skia_frames.push(img);
                } else {
                    let mut png_bytes = Vec::new();
                    resized.write_to(&mut Cursor::new(&mut png_bytes), Png)?;
                    let data_e = Data::new_copy(&png_bytes);
                    if let Some(img) = Image::from_encoded(&data_e) {
                        let delay_ms = match frame.delay().numer_denom_ms() {
                            (numer, denom) if denom != 0 => numer / denom,
                            (numer, _) => numer,
                        };
                        durations_ms.push(delay_ms);
                        skia_frames.push(img);
                    } else {
                        return Err(AppError::Skia(
                            "Skia failed to decode gif fallback frame".into(),
                        ));
                    }
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
                frames: skia_frames,
                durations_ms,
                cum_durations: cum,
                total_ms,
                w,
                h,
            })
        }
        _ => {
            let dyn_img = image::ImageReader::new(Cursor::new(bytes.to_vec()))
                .with_guessed_format()?
                .decode()?;
            let resized = resize_dynamic_image_preserve_aspect(dyn_img, target_h);
            let rgba = resized.to_rgba8();
            let (w, h) = rgba.dimensions();
            let row_bytes = (w * 4) as usize;
            let data = Data::new_copy(rgba.as_raw());

            // FIXED: Using AlphaType::Unpremul
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
                Ok(EmoteData::Static {
                    img,
                    w: w as i32,
                    h: h as i32,
                })
            } else {
                let mut png_bytes = Vec::new();
                resized.write_to(&mut Cursor::new(&mut png_bytes), Png)?;
                let data2 = Data::new_copy(&png_bytes);
                if let Some(img2) = Image::from_encoded(&data2) {
                    let w = img2.width();
                    let h = img2.height();
                    Ok(EmoteData::Static { img: img2, w, h })
                } else {
                    Err(AppError::EmoteCache(
                        "failed to decode static emote".to_owned(),
                    ))
                }
            }
        }
    }
}