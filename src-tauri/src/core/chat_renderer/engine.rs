use crate::core::AppTask;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use skia_safe::{
    surfaces, AlphaType, Color, ColorType, Font, FontMgr, FontStyle, Image, ImageInfo, Paint,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;

use crate::core::chat::types::UnifiedChatMessage;
use crate::core::chat_renderer::args::{BackgroundMode, EvictionStrategy, RenderVideoArgs};
use crate::core::chat_renderer::helpers::ease_out;
use crate::core::chat_renderer::regex::{EMOTE_REGEX, IMAGE_URL_REGEX};
use crate::core::chat_renderer::types::{EmoteCache, EmoteData, ImageCache};
use crate::error::AppError;
use crate::types::AppResult;

const EMOTE_SCALE: f32 = 1.15;
const PRE_RENDER_BATCH_SIZE: usize = 64;

// 1. Thread Pool for Parallel Computations
static RENDER_POOL: once_cell::sync::Lazy<rayon::ThreadPool> = once_cell::sync::Lazy::new(|| {
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let workers = (available * 3 / 4)
        .max(1)
        .min(available.saturating_sub(2).max(1));
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|i| format!("engine-worker-{}", i))
        .build()
        .expect("Failed to build render thread pool")
});

// 2. Performance Caches & Memory Recycling Pools
thread_local! {
    static SKIA_SURFACE: RefCell<Option<skia_safe::Surface>> = RefCell::new(None);
    static PRE_RENDER_MEASURE_CACHE: RefCell<FxHashMap<String, f32>> = RefCell::new(FxHashMap::default());
}

#[derive(Default)]
struct PixelBufferPool {
    inner: Mutex<Vec<Vec<u8>>>,
}

impl PixelBufferPool {
    fn acquire(&self, min_len: usize) -> Vec<u8> {
        let mut guard = self.inner.lock().unwrap();
        let mut buf = guard.pop().unwrap_or_else(|| Vec::with_capacity(min_len));
        if buf.capacity() < min_len {
            buf.reserve(min_len - buf.capacity());
        }
        buf.clear();
        buf
    }
    fn release(&self, mut buf: Vec<u8>) {
        buf.clear();
        self.inner.lock().unwrap().push(buf);
    }
}

// 3. Layout Structs
#[derive(Clone)]
struct ScheduledMessage {
    spawn_frame: u32,
    img: Option<Image>,
    img_h: Option<i32>,
    placements: Option<Vec<EmotePlacement>>,
}

#[derive(Clone)]
pub struct EmotePlacement {
    emote_id: Option<i32>,
    media_url: Option<String>,
    x: f32,
    y: f32,
    w: i32,
    h: i32,
    animated: bool,
}

// 4. Core Render Pipeline
#[allow(clippy::too_many_arguments)]
fn render_message_to_image_blocking(
    content: &str,
    username: &str,
    user_hex_color: &str,
    username_font: &Font,
    message_font: &Font,
    message_color: Color,
    available_w: f32,
    msg_line_h: f32,
    message_ascent: f32,
    emote_cache: &EmoteCache,
    image_cache: &ImageCache,
    args: &RenderVideoArgs,
    measure_cache: &mut FxHashMap<String, f32>,
) -> Result<(Image, i32, i32, Vec<EmotePlacement>), AppError> {
    #[derive(Clone, Copy)]
    enum Segment<'a> {
        Text(&'a str),
        Emote(i32, &'a str),
        Media(&'a str),
    }

    #[inline]
    fn measure_cached(font: &Font, key: &str, cache: &mut FxHashMap<String, f32>) -> f32 {
        if let Some(&v) = cache.get(key) {
            return v;
        }
        let (w, _) = font.measure_str(key, None);
        cache.insert(key.to_owned(), w);
        w
    }

    fn split_into_fragments<'a>(
        input: &'a str,
        font: &Font,
        max_w: f32,
        cache: &mut FxHashMap<String, f32>,
    ) -> Vec<&'a str> {
        let mut out = Vec::new();
        let mut start = 0;
        while start < input.len() {
            let remainder = &input[start..];
            if measure_cached(font, remainder, cache) <= max_w {
                out.push(remainder);
                break;
            }
            let mut boundaries = Vec::with_capacity(remainder.chars().count() + 1);
            for (idx, _) in remainder.char_indices() {
                boundaries.push(idx);
            }
            boundaries.push(remainder.len());

            let mut lo = 1usize;
            let mut hi = boundaries.len().saturating_sub(1);
            let mut best = boundaries[1].max(1);

            while lo <= hi {
                let mid = (lo + hi) / 2;
                if measure_cached(font, &remainder[..boundaries[mid]], cache) <= max_w {
                    best = boundaries[mid].max(1);
                    lo = mid + 1;
                } else {
                    if mid == 0 {
                        break;
                    }
                    hi = mid - 1;
                }
            }
            out.push(&remainder[..best]);
            start += best;
        }
        out
    }

    let mut matches = Vec::new();
    for caps in EMOTE_REGEX.captures_iter(content) {
        if let (Some(m), Some(id_cap), Some(name_cap)) =
            (caps.get(0), caps.name("id"), caps.name("name"))
        {
            let id = id_cap.as_str().parse::<i32>().unwrap_or(0);
            matches.push((m.start(), m.end(), Segment::Emote(id, name_cap.as_str())));
        }
    }
    for m in IMAGE_URL_REGEX.find_iter(content) {
        matches.push((m.start(), m.end(), Segment::Media(m.as_str())));
    }
    matches.sort_by_key(|a| a.0);

    let mut segments = Vec::new();
    let mut last = 0;
    for (start, end, seg) in matches {
        if start < last {
            continue;
        }
        if start > last {
            segments.push(Segment::Text(&content[last..start]));
        }
        segments.push(seg);
        last = end;
    }
    if last < content.len() {
        segments.push(Segment::Text(&content[last..]));
    }

    let max_w = available_w.max(1.0);
    let prefix = format!("{}: ", username);
    let prefix_w = measure_cached(username_font, &prefix, measure_cache);
    let space_w = measure_cached(message_font, " ", measure_cache);

    let mut lines: Vec<Vec<Segment>> = Vec::new();
    let mut current_line = Vec::new();
    let mut cur_w = prefix_w;

    for seg in &segments {
        match seg {
            Segment::Text(s) => {
                for (pi, para) in s.split('\n').enumerate() {
                    if para.is_empty() && pi > 0 {
                        lines.push(std::mem::take(&mut current_line));
                        cur_w = 0.0;
                        continue;
                    }
                    let mut first_word = true;
                    for raw_word in para.split_whitespace() {
                        let word_w = measure_cached(message_font, raw_word, measure_cache);
                        let needed_space = if first_word { 0.0 } else { space_w };

                        if cur_w + needed_space + word_w <= max_w {
                            if !first_word {
                                current_line.push(Segment::Text(" "));
                                cur_w += space_w;
                            }
                            current_line.push(Segment::Text(raw_word));
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
                                    max_w,
                                    measure_cache,
                                );
                                let flen = frags.len();
                                for (fi, f) in frags.into_iter().enumerate() {
                                    current_line.push(Segment::Text(f));
                                    cur_w += measure_cached(message_font, f, measure_cache);
                                    if fi < flen - 1 {
                                        lines.push(std::mem::take(&mut current_line));
                                        cur_w = 0.0;
                                    }
                                }
                            } else {
                                current_line.push(Segment::Text(raw_word));
                                cur_w = word_w;
                            }
                        }
                        first_word = false;
                    }
                }
            }
            Segment::Emote(id, _) => {
                let ew = emote_cache
                    .get(*id)
                    .map(|ed| ed.width() as f32)
                    .unwrap_or(emote_cache.target_height() as f32)
                    * EMOTE_SCALE;
                if cur_w + ew > max_w && !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                    cur_w = 0.0;
                }
                current_line.push(*seg);
                cur_w += ew;
            }
            Segment::Media(url) => {
                let mw = image_cache
                    .get(url)
                    .map(|ed| ed.width() as f32)
                    .unwrap_or(image_cache.target_height() as f32)
                    * EMOTE_SCALE;
                if cur_w + mw > max_w && !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                    cur_w = 0.0;
                }
                current_line.push(*seg);
                cur_w += mw;
            }
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    let mut measured_max_w = 0f32;
    let mut line_heights = Vec::with_capacity(lines.len());
    for (li, line) in lines.iter().enumerate() {
        let mut lw = if li == 0 { prefix_w } else { 0.0 };
        let mut lh = msg_line_h;
        for seg in line {
            match seg {
                Segment::Text(s) => lw += measure_cached(message_font, s, measure_cache),
                Segment::Emote(id, _) => {
                    let (w, h) = emote_cache
                        .get(*id)
                        .map(|ed| (ed.width() as f32, ed.height() as f32))
                        .unwrap_or((
                            emote_cache.target_height() as f32,
                            emote_cache.target_height() as f32,
                        ));
                    lw += w * EMOTE_SCALE;
                    lh = lh.max(h * EMOTE_SCALE);
                }
                Segment::Media(url) => {
                    let (w, h) = image_cache
                        .get(url)
                        .map(|ed| (ed.width() as f32, ed.height() as f32))
                        .unwrap_or((
                            image_cache.target_height() as f32,
                            image_cache.target_height() as f32,
                        ));
                    lw += w * EMOTE_SCALE;
                    lh = lh.max((h * EMOTE_SCALE) + 8.0);
                }
            }
        }
        measured_max_w = measured_max_w.max(lw);
        line_heights.push(lh);
    }

    let bubble_pad = args.bubble_padding.max(0) as f32;
    let content_width = (measured_max_w + bubble_pad * 2.0).ceil() as i32;
    let final_width = if args.bubble_mode_full_width {
        (max_w.ceil() as i32).max(1)
    } else {
        content_width.max(1)
    };
    let final_height = (line_heights.iter().sum::<f32>() + bubble_pad * 2.0).ceil() as i32;

    SKIA_SURFACE.with(|surf_cell| {
        let mut surf_opt = surf_cell.borrow_mut();
        if surf_opt.is_none()
            || surf_opt.as_ref().unwrap().width() != final_width
            || surf_opt.as_ref().unwrap().height() != final_height
        {
            *surf_opt = Some(
                surfaces::raster_n32_premul((final_width, final_height))
                    .expect("Raster Allocation Failure"),
            );
        }
        let surf = surf_opt.as_mut().unwrap();
        let canvas = surf.canvas();
        canvas.clear(Color::TRANSPARENT);

        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_color(Color::from(&args.bubble_color));
        canvas.draw_round_rect(
            skia_safe::Rect::new(0.0, 0.0, final_width as f32, final_height as f32),
            args.bubble_radius,
            args.bubble_radius,
            &paint,
        );

        let mut placements = Vec::new();
        let mut y_cursor = bubble_pad;

        let parsed_user_color = if !user_hex_color.is_empty() {
            let clean = user_hex_color.trim_start_matches('#');
            if let Ok(val) = u32::from_str_radix(clean, 16) {
                Color::from_rgb((val >> 16) as u8, (val >> 8) as u8, val as u8)
            } else {
                Color::WHITE
            }
        } else {
            Color::WHITE
        };

        for (li, line) in lines.iter().enumerate() {
            let lh = line_heights[li];
            let baseline = y_cursor + ((lh - msg_line_h) / 2.0).max(0.0) - message_ascent;
            let mut x_cursor = bubble_pad;

            if li == 0 {
                if args.username_shadow {
                    paint.set_color(Color::from_argb(180, 0, 0, 0));
                    canvas.draw_str(
                        &prefix,
                        (x_cursor + 2.0, baseline + 2.0),
                        username_font,
                        &paint,
                    );
                }
                if args.outline_usernames {
                    paint.set_style(skia_safe::paint::Style::Stroke);
                    paint.set_stroke_width(args.username_outline_width.unwrap_or(1.5));
                    paint.set_color(Color::from_argb(200, 0, 0, 0));
                    canvas.draw_str(&prefix, (x_cursor, baseline), username_font, &paint);
                    paint.set_style(skia_safe::paint::Style::Fill);
                }
                paint.set_color(parsed_user_color);
                canvas.draw_str(&prefix, (x_cursor, baseline), username_font, &paint);
                x_cursor += prefix_w;
            }

            for seg in line {
                match seg {
                    Segment::Text(s) => {
                        paint.set_color(message_color);
                        canvas.draw_str(s, (x_cursor, baseline), message_font, &paint);
                        x_cursor += measure_cached(message_font, s, measure_cache);
                    }
                    Segment::Emote(id, _) => {
                        if let Some(ed) = emote_cache.get(*id) {
                            let sw = ed.width() as f32 * EMOTE_SCALE;
                            let sh = ed.height() as f32 * EMOTE_SCALE;
                            let draw_y = if args.center_emotes_vertically {
                                y_cursor + (lh - sh) / 2.0
                            } else {
                                y_cursor
                            };

                            let is_animated = match &*ed {
                                EmoteData::Animated { .. } => true,
                                EmoteData::Static { img, .. } => {
                                    canvas.save();
                                    canvas.translate((x_cursor, draw_y));
                                    canvas.scale((EMOTE_SCALE, EMOTE_SCALE));
                                    canvas.draw_image(img, (0, 0), None);
                                    canvas.restore();
                                    false
                                }
                            };
                            placements.push(EmotePlacement {
                                emote_id: Some(*id),
                                media_url: None,
                                x: x_cursor,
                                y: draw_y,
                                w: sw as i32,
                                h: sh as i32,
                                animated: is_animated,
                            });
                            x_cursor += sw;
                        }
                    }
                    Segment::Media(url) => {
                        if let Some(ed) = image_cache.get(url) {
                            let sw = ed.width() as f32 * EMOTE_SCALE;
                            let sh = ed.height() as f32 * EMOTE_SCALE;
                            let draw_y = if args.center_emotes_vertically {
                                y_cursor + (lh - sh) / 2.0
                            } else {
                                y_cursor
                            };

                            let is_animated = match &*ed {
                                EmoteData::Animated { .. } => true,
                                EmoteData::Static { img, .. } => {
                                    canvas.save();
                                    canvas.translate((x_cursor, draw_y));
                                    canvas.scale((EMOTE_SCALE, EMOTE_SCALE));
                                    canvas.draw_image(img, (0, 0), None);
                                    canvas.restore();
                                    false
                                }
                            };
                            placements.push(EmotePlacement {
                                emote_id: None,
                                media_url: Some(url.to_string()),
                                x: x_cursor,
                                y: draw_y,
                                w: sw as i32,
                                h: sh as i32,
                                animated: is_animated,
                            });
                            x_cursor += sw;
                        }
                    }
                }
            }
            y_cursor += lh;
        }
        Ok((surf.image_snapshot(), final_width, final_height, placements))
    })
}

// 5. Main Engine Execution Process
pub async fn process_chat_render(
    app: &AppHandle,
    tasks: Arc<Mutex<HashMap<String, AppTask>>>,
    task_id: &str,
    input_path: PathBuf,
    args: RenderVideoArgs,
    cache_dir_base: PathBuf,
    cancel_flag: Arc<AtomicBool>,
) -> AppResult<()> {
    // --- Helper to update task progress ---
    let emit_progress = |progress: f32, text: &str| {
        let mut locked = tasks.lock().unwrap();
        if let Some(task) = locked.get_mut(task_id) {
            task.progress = progress;
            task.status_text = Some(text.to_string());
            let _ = app.emit("task-progress", task.clone());
        }
    };

    emit_progress(1.0, "Preparing FFmpeg and Scanning Metadata...");

    // 1. DYNAMIC FFMPEG CONFIGURATION
    let (vcodec, pix_fmt, extension) = match args.background_mode {
        BackgroundMode::Transparent => ("prores_ks", "yuva444p10le", "mov"),
        _ => {
            let has_nvenc = Command::new("ffmpeg")
                .args(&["-h", "encoder=h264_nvenc"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if has_nvenc {
                ("h264_nvenc", "yuv420p", "mp4")
            } else {
                ("libx264", "yuv420p", "mp4")
            }
        }
    };

    let raw_output_path = Path::new(&args.output_path);
    let output_file_str = raw_output_path
        .with_extension(extension)
        .to_string_lossy()
        .to_string();

    let mut ffmpeg_args = vec![
        "-y".to_string(),
        "-f".to_string(),
        "rawvideo".to_string(),
        "-pix_fmt".to_string(),
        "rgba".to_string(),
        "-s".to_string(),
        format!("{}x{}", args.width, args.height),
        "-r".to_string(),
        args.fps.to_string(),
        "-i".to_string(),
        "-".to_string(),
        "-c:v".to_string(),
        vcodec.to_string(),
        "-pix_fmt".to_string(),
        pix_fmt.to_string(),
    ];

    if vcodec == "prores_ks" {
        ffmpeg_args.extend(vec!["-profile:v".to_string(), "4444".to_string()]);
    } else if vcodec == "h264_nvenc" {
        ffmpeg_args.extend(vec![
            "-preset".to_string(),
            "p4".to_string(),
            "-cq".to_string(),
            "20".to_string(),
        ]);
    } else {
        ffmpeg_args.extend(vec![
            "-preset".to_string(),
            "fast".to_string(),
            "-crf".to_string(),
            "20".to_string(),
        ]);
    }
    ffmpeg_args.push(output_file_str.clone());

    let mut ffmpeg_child = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let mut ff_stdin = ffmpeg_child.stdin.take().unwrap();

    // 2. PASS 1: SCAN THE JSONL FOR CACHING & TIMELINE LIMITS
    let file = File::open(&input_path).await?;
    let mut reader = tokio::io::BufReader::new(file).lines();

    let mut emote_ids = HashSet::new();
    let mut image_urls = HashSet::new();
    let mut max_offset_sec: f64 = 0.0;
    let skip_users_set: HashSet<String> = args.skip_users.clone().into_iter().collect();

    while let Some(line) = reader.next_line().await? {
        if let Ok(msg) = serde_json::from_str::<UnifiedChatMessage>(&line) {
            if skip_users_set.contains(&msg.username) {
                continue;
            }
            max_offset_sec = max_offset_sec.max(msg.offset_sec);
            for caps in EMOTE_REGEX.captures_iter(&msg.content) {
                if let Some(id) = caps.name("id").and_then(|m| m.as_str().parse::<i32>().ok()) {
                    emote_ids.insert(id);
                }
            }
            for mat in IMAGE_URL_REGEX.find_iter(&msg.content) {
                image_urls.insert(mat.as_str().to_string());
            }
        }
    }

    emit_progress(5.0, "Hydrating caches...");

    // 3. CACHE HYDRATION
    let target_emote_h = ((args.font_size + args.line_spacing as f32) * 0.85).ceil() as u32;
    let emote_cache = Arc::new(EmoteCache::new(
        cache_dir_base.join("emote_cache"),
        512,
        target_emote_h,
    ));
    let img_cache = Arc::new(ImageCache::new(
        cache_dir_base.join("image_cache"),
        128,
        target_emote_h * 4,
    ));

    emote_cache
        .ensure_cached(&emote_ids.into_iter().collect::<Vec<_>>())
        .await?;
    img_cache
        .ensure_cached(&image_urls.into_iter().collect::<Vec<_>>())
        .await?;

    // Font Configuration
    let font_mgr = FontMgr::new();
    let typeface = font_mgr
        .match_family_style(&args.font_name, FontStyle::normal())
        .or_else(|| font_mgr.match_family_style("Arial", FontStyle::normal()))
        .or_else(|| font_mgr.match_family_style("Helvetica", FontStyle::normal()))
        .ok_or_else(|| AppError::InternalError("No system fonts found".into()))?;

    let message_font = Font::from_typeface(typeface.clone(), args.font_size);
    let username_font = Font::from_typeface(typeface, (args.font_size * 0.95).max(12.0));
    let (_, metrics) = message_font.metrics();
    let msg_line_h = (metrics.descent - metrics.ascent) + args.line_spacing as f32;

    // 4. PIPELINE SETUP (Memory safe streaming)
    let (loader_tx, loader_rx) = crossbeam_channel::bounded::<UnifiedChatMessage>(2048);
    let (stamp_tx, stamp_rx) = crossbeam_channel::bounded::<(u32, Arc<ScheduledMessage>)>(512);

    // THREAD A: Loader Thread (Reads file safely without blowing up memory)
    let loader_path = input_path.clone();
    let loader_cancel = Arc::clone(&cancel_flag);
    std::thread::spawn(move || {
        let f = std::fs::File::open(loader_path).unwrap();
        let reader = std::io::BufReader::new(f);
        for line in std::io::BufRead::lines(reader).flatten() {
            if loader_cancel.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(msg) = serde_json::from_str::<UnifiedChatMessage>(&line) {
                if !skip_users_set.contains(&msg.username) {
                    if loader_tx.send(msg).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // THREAD B: Pre-render Thread (Batches and computes text layouts dynamically)
    let args_pr = args.clone();
    let emote_cache_pr = Arc::clone(&emote_cache);
    let img_cache_pr = Arc::clone(&img_cache);
    let pr_cancel = Arc::clone(&cancel_flag);

    std::thread::spawn(move || {
        let mut batch = Vec::with_capacity(PRE_RENDER_BATCH_SIZE);
        let mut last_assigned_frame = -1i64;

        let flush_batch = |batch_to_flush: Vec<UnifiedChatMessage>, last_frame: &mut i64| {
            if batch_to_flush.is_empty() {
                return;
            }

            // Process text layouts concurrently
            let rendered: Vec<Option<ScheduledMessage>> = RENDER_POOL.install(|| {
                batch_to_flush
                    .into_par_iter()
                    .map(|msg| {
                        PRE_RENDER_MEASURE_CACHE.with(|cache_cell| {
                            let mut measure_cache = cache_cell.borrow_mut();

                            // ANTI-CLUMPING LOGIC: Spread messages out so they don't overlap completely
                            let base_frame = (msg.offset_sec * args_pr.fps as f64).round() as i64;
                            let assigned_frame = if base_frame <= *last_frame {
                                *last_frame + 2
                            } else {
                                base_frame
                            }; // Minimum 2 frames apart

                            match render_message_to_image_blocking(
                                &msg.content,
                                &msg.username,
                                &msg.color,
                                &username_font,
                                &message_font,
                                Color::from(&args_pr.message_color),
                                (args_pr.width - 2 * args_pr.padding) as f32,
                                msg_line_h,
                                metrics.ascent,
                                &emote_cache_pr,
                                &img_cache_pr,
                                &args_pr,
                                &mut *measure_cache,
                            ) {
                                Ok((img, _w, h, placements)) => Some(ScheduledMessage {
                                    spawn_frame: assigned_frame as u32,
                                    img: Some(img),
                                    img_h: Some(h),
                                    placements: Some(placements),
                                }),
                                Err(_) => None,
                            }
                        })
                    })
                    .collect()
            });

            for sched in rendered.into_iter().flatten() {
                *last_frame = sched.spawn_frame as i64;
                if stamp_tx.send((sched.spawn_frame, Arc::new(sched))).is_err() {
                    return;
                }
            }
        };

        while let Ok(msg) = loader_rx.recv() {
            if pr_cancel.load(Ordering::SeqCst) {
                break;
            }
            batch.push(msg);
            if batch.len() >= PRE_RENDER_BATCH_SIZE {
                flush_batch(std::mem::take(&mut batch), &mut last_assigned_frame);
            }
        }
        flush_batch(batch, &mut last_assigned_frame);
    });

    // 5. COORDINATOR LOOP (Assembles chunks, pushes to FFmpeg)
    let total_frames = ((max_offset_sec * args.fps as f64).round() as u32)
        + (args.message_hold_seconds * args.fps);

    let bg_color = match args.background_mode {
        BackgroundMode::Transparent => Color::TRANSPARENT,
        BackgroundMode::ChromaKeyGreen => Color::from_argb(255, 0, 255, 0),
        BackgroundMode::CustomColor => Color::from(&args.background_color),
    };

    let info = ImageInfo::new(
        (args.width, args.height),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let num_bytes = (args.width * args.height * 4) as usize;
    let pixel_pool = Arc::new(PixelBufferPool::default());

    let chunk_size = (args.fps as usize * 2).max(30);
    let mut frame_chunk: Vec<(u32, Vec<Arc<ScheduledMessage>>)> = Vec::with_capacity(chunk_size);
    let mut active_bubbles: VecDeque<Arc<ScheduledMessage>> = VecDeque::new();
    let mut next_stamp: Option<(u32, Arc<ScheduledMessage>)> = None;

    emit_progress(10.0, "Rendering frames...");

    for f_idx in 0..total_frames {
        if cancel_flag.load(Ordering::SeqCst) {
            break;
        }

        // Only consume the exact layout items required for this precise frame tick
        loop {
            if next_stamp.is_none() {
                next_stamp = stamp_rx.try_recv().ok();
            }
            if let Some((spawn_frame, sched)) = &next_stamp {
                if *spawn_frame <= f_idx {
                    active_bubbles.push_front(sched.clone());
                    next_stamp = None;
                    continue;
                }
            }
            break;
        }

        // Apply automatic eviction rules (this is what deletes the unused bitmaps from RAM)
        active_bubbles.retain(|bubble| match args.eviction_strategy {
            EvictionStrategy::Timed => {
                let age_secs = (f_idx - bubble.spawn_frame) as f32 / args.fps as f32;
                age_secs <= (args.message_hold_seconds + args.message_fade_out_seconds) as f32
            }
            EvictionStrategy::PushOnly => true,
        });

        frame_chunk.push((
            f_idx,
            active_bubbles.iter().map(|b| Arc::clone(b)).collect(),
        ));

        if frame_chunk.len() >= chunk_size || f_idx == total_frames - 1 {
            let pool = Arc::clone(&pixel_pool);
            let args_block = args.clone();
            let rayon_emotes = Arc::clone(&emote_cache);
            let rayon_imgs = Arc::clone(&img_cache);
            let info_clone = info.clone();

            let current_chunk = std::mem::take(&mut frame_chunk);

            // CPU Thread Pool Assembly Line
            let rendered_pixels: Vec<Vec<u8>> = RENDER_POOL.install(|| {
                current_chunk
                    .into_par_iter()
                    .map(move |(frame_id, bubbles)| {
                        let mut pixels = pool.acquire(num_bytes);
                        unsafe {
                            pixels.set_len(num_bytes);
                        }

                        SKIA_SURFACE.with(|surf_cell| {
                            let mut surf_opt = surf_cell.borrow_mut();
                            if surf_opt.is_none()
                                || surf_opt.as_ref().unwrap().width() != args_block.width
                                || surf_opt.as_ref().unwrap().height() != args_block.height
                            {
                                *surf_opt = Some(
                                    surfaces::raster_n32_premul((
                                        args_block.width,
                                        args_block.height,
                                    ))
                                    .unwrap(),
                                );
                            }

                            let surface = surf_opt.as_mut().unwrap();
                            let canvas = surface.canvas();
                            canvas.clear(bg_color);

                            let mut y_cursor = (args_block.height - args_block.padding) as f32;
                            let mut paint = Paint::default();
                            paint.set_anti_alias(true);

                            for bubble in bubbles.iter() {
                                let age_secs =
                                    (frame_id - bubble.spawn_frame) as f32 / args_block.fps as f32;
                                let mut alpha = 1.0;

                                if args_block.anim_fade_in {
                                    let intro = 0.5;
                                    if age_secs < intro {
                                        alpha *= age_secs / intro;
                                    }
                                }
                                if matches!(args_block.eviction_strategy, EvictionStrategy::Timed) {
                                    let fade_start = args_block.message_hold_seconds as f32;
                                    if age_secs > fade_start {
                                        alpha *= 1.0
                                            - ((age_secs - fade_start)
                                                / args_block.message_fade_out_seconds as f32)
                                                .clamp(0.0, 1.0);
                                    }
                                }

                                paint.set_alpha((255.0 * alpha) as u8);
                                canvas.save();

                                let img_h = bubble.img_h.unwrap_or(0);
                                let top = y_cursor - img_h as f32;
                                let final_x = args_block.padding as f32;

                                let x_translate = if args_block.anim_slide && age_secs < 0.5 {
                                    let eased = ease_out(age_secs / 0.5);
                                    final_x + (1.0 - eased) * (args_block.width as f32 - final_x)
                                } else {
                                    final_x
                                };

                                canvas.translate((x_translate, top));

                                if let Some(img) = &bubble.img {
                                    canvas.draw_image(img, (0, 0), Some(&paint));
                                }

                                if let Some(placements) = &bubble.placements {
                                    for p in placements {
                                        if p.animated {
                                            let ed_opt = if let Some(id) = p.emote_id {
                                                rayon_emotes.get(id)
                                            } else {
                                                p.media_url
                                                    .as_ref()
                                                    .and_then(|url| rayon_imgs.get(url))
                                            };
                                            if let Some(ed) = ed_opt {
                                                let t_ms = (frame_id as u64 * 1000)
                                                    / args_block.fps as u64;
                                                if let Some(fi) = ed.frame_at(t_ms) {
                                                    canvas.save();
                                                    canvas.scale((EMOTE_SCALE, EMOTE_SCALE));
                                                    canvas.draw_image(
                                                        fi,
                                                        (p.x / EMOTE_SCALE, p.y / EMOTE_SCALE),
                                                        Some(&paint),
                                                    );
                                                    canvas.restore();
                                                }
                                            }
                                        }
                                    }
                                }

                                canvas.restore();
                                y_cursor -= img_h as f32 + args_block.message_spacing as f32;
                                if y_cursor < 0.0 {
                                    break;
                                } // Spatially Evicted
                            }
                            surface.read_pixels(
                                &info_clone,
                                pixels.as_mut_slice(),
                                (args_block.width * 4) as usize,
                                (0, 0),
                            );
                        });
                        pixels
                    })
                    .collect()
            });

            // Write all frames in deterministic order to FFmpeg
            for frame_pixels in rendered_pixels {
                let _ = ff_stdin.write_all(&frame_pixels);
                pixel_pool.release(frame_pixels);
            }

            // Report Progress Upward
            let progress_percent = 10.0 + ((f_idx as f32 / total_frames as f32) * 90.0);
            emit_progress(
                progress_percent,
                &format!("Rendering Video... ({:.1}%)", progress_percent),
            );
        }
    }

    drop(ff_stdin); // Signifies to FFmpeg we are done writing

    if cancel_flag.load(Ordering::SeqCst) {
        emit_progress(100.0, "Render Cancelled");
        let _ = ffmpeg_child.kill();
        let _ = ffmpeg_child.wait();
        return Err(AppError::InternalError("Cancelled by user".into()));
    } else {
        emit_progress(100.0, "Finishing Encoding...");
        let _ = ffmpeg_child.wait();
        emit_progress(100.0, "Complete");
    }

    Ok(())
}
