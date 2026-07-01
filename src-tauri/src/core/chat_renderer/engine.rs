use crate::core::AppTask;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use skia_safe::{
    surfaces, AlphaType, Color, ColorType, Font, FontMgr, FontStyle, ImageInfo, Paint, TextBlob,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use stream_extractor::MessageSaved;
use tauri::{AppHandle, Emitter};
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;

use crate::core::chat_renderer::args::{BackgroundMode, EvictionStrategy, RenderVideoArgs};
use crate::core::chat_renderer::emote_providers::{tokenise, MessageToken, ResolvedEmote};
use crate::core::chat_renderer::helpers::ease_out;
use crate::core::chat_renderer::types::{
    EmoteCache, EmoteData, ImageCache, LayoutLine, LayoutToken,
};
use crate::error::AppError;
use crate::types::AppResult;

const EMOTE_SCALE: f32 = 1.25;
const EMOTE_MARGIN: f32 = 6.0;
const PRE_RENDER_BATCH_SIZE: usize = 64;

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

#[derive(Clone)]
struct ScheduledMessage {
    spawn_frame: u32,
    lines: Vec<LayoutLine>,
    bubble_w: i32,
    bubble_h: i32,
    bg_color: Color,
    user_color: Color,
    is_grouped: bool,
}

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
    measure_cache: &mut FxHashMap<String, f32>,
    is_grouped: bool,
) -> Result<(Vec<LayoutLine>, i32, i32, Color), AppError> {
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

    let tokens = tokenise(content, None);
    let max_w = available_w.max(1.0);
    let prefix = format!("{}: ", username);
    let prefix_w = measure_cached(username_font, &prefix, measure_cache);
    let space_w = measure_cached(message_font, " ", measure_cache);

    let mut lines: Vec<Vec<MessageToken>> = Vec::new();
    let mut current_line = Vec::new();
    let mut cur_w = if is_grouped { 0.0 } else { prefix_w };

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
                    for raw_word in para.split_whitespace() {
                        let word_w = measure_cached(message_font, raw_word, measure_cache);
                        let needed_space = if first_word { 0.0 } else { space_w };

                        if cur_w + needed_space + word_w <= max_w {
                            if !first_word {
                                current_line.push(MessageToken::Text(" ".into()));
                                cur_w += space_w;
                            }
                            current_line.push(MessageToken::Text(raw_word.into()));
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
                                    current_line.push(MessageToken::Text(f.into()));
                                    cur_w += measure_cached(message_font, f, measure_cache);
                                    if fi < flen - 1 {
                                        lines.push(std::mem::take(&mut current_line));
                                        cur_w = 0.0;
                                    }
                                }
                            } else {
                                current_line.push(MessageToken::Text(raw_word.into()));
                                cur_w = word_w;
                            }
                        }
                        first_word = false;
                    }
                }
            }
            MessageToken::KickEmote { id, .. } => {
                let parsed_id = id.parse::<i32>().unwrap_or(0);
                let ew = emote_cache
                    .get(parsed_id)
                    .map(|ed| ed.width() as f32)
                    .unwrap_or(emote_cache.target_height() as f32)
                    * EMOTE_SCALE;
                let padded_ew = ew + EMOTE_MARGIN;

                if cur_w + padded_ew > max_w && !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                    cur_w = 0.0;
                }
                current_line.push(token.clone());
                cur_w += padded_ew;
            }
            MessageToken::ProviderEmote(ResolvedEmote { url, .. })
            | MessageToken::ImageUrl(url) => {
                let mw = image_cache
                    .get(url)
                    .map(|ed| ed.width() as f32)
                    .unwrap_or(image_cache.target_height() as f32)
                    * EMOTE_SCALE;
                let padded_mw = mw + EMOTE_MARGIN;

                if cur_w + padded_mw > max_w && !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                    cur_w = 0.0;
                }
                current_line.push(token.clone());
                cur_w += padded_mw;
            }
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    let bubble_pad = args.bubble_padding.max(0) as f32;
    let mut layout_lines = Vec::with_capacity(lines.len());
    let mut measured_max_w = 0f32;
    let mut y_cursor = bubble_pad;

    for (li, line) in lines.iter().enumerate() {
        let mut lw = if li == 0 && !is_grouped {
            prefix_w
        } else {
            0.0
        };
        let mut lh = msg_line_h;

        for token in line {
            match token {
                MessageToken::Text(s) => lw += measure_cached(message_font, s, measure_cache),
                MessageToken::KickEmote { id, .. } => {
                    let parsed_id = id.parse::<i32>().unwrap_or(0);
                    let (_, h) = emote_cache
                        .get(parsed_id)
                        .map(|ed| (ed.width() as f32, ed.height() as f32))
                        .unwrap_or((0.0, emote_cache.target_height() as f32));
                    lh = lh.max(h * EMOTE_SCALE);
                }
                MessageToken::ProviderEmote(ResolvedEmote { url, .. })
                | MessageToken::ImageUrl(url) => {
                    let (_, h) = image_cache
                        .get(url)
                        .map(|ed| (ed.width() as f32, ed.height() as f32))
                        .unwrap_or((0.0, image_cache.target_height() as f32));
                    lh = lh.max((h * EMOTE_SCALE) + 8.0);
                }
            }
        }

        let baseline = y_cursor + ((lh - msg_line_h) / 2.0).max(0.0) - message_ascent;
        let mut x_cursor = bubble_pad;
        let mut layout_tokens = Vec::new();

        if li == 0 && !is_grouped {
            if let Some(blob) = TextBlob::from_str(&prefix, username_font) {
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
                    let w = measure_cached(message_font, s, measure_cache);
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
                MessageToken::KickEmote { id, .. } => {
                    let parsed_id = id.parse::<i32>().unwrap_or(0);
                    if let Some(ed) = emote_cache.get(parsed_id) {
                        let sw = ed.width() as f32 * EMOTE_SCALE;
                        let sh = ed.height() as f32 * EMOTE_SCALE;
                        let draw_y = if args.center_emotes_vertically {
                            y_cursor + (lh - sh) / 2.0
                        } else {
                            y_cursor
                        };
                        let draw_x = x_cursor + (EMOTE_MARGIN / 2.0);

                        layout_tokens.push(LayoutToken::Emote {
                            data: ed,
                            x: draw_x,
                            y: draw_y,
                        });
                        x_cursor += sw + EMOTE_MARGIN;
                    }
                }
                MessageToken::ProviderEmote(ResolvedEmote { url, .. })
                | MessageToken::ImageUrl(url) => {
                    if let Some(ed) = image_cache.get(url) {
                        let sw = ed.width() as f32 * EMOTE_SCALE;
                        let sh = ed.height() as f32 * EMOTE_SCALE;
                        let draw_y = if args.center_emotes_vertically {
                            y_cursor + (lh - sh) / 2.0
                        } else {
                            y_cursor
                        };
                        let draw_x = x_cursor + (EMOTE_MARGIN / 2.0);

                        layout_tokens.push(LayoutToken::Emote {
                            data: ed,
                            x: draw_x,
                            y: draw_y,
                        });
                        x_cursor += sw + EMOTE_MARGIN;
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

pub async fn process_chat_render(
    app: &AppHandle,
    tasks: Arc<Mutex<HashMap<String, AppTask>>>,
    task_id: &str,
    input_path: PathBuf,
    args: RenderVideoArgs,
    cache_dir_base: PathBuf,
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

    emit_progress(1.0, "Preparing FFmpeg and Scanning Metadata...");

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

    let is_luma = matches!(args.background_mode, BackgroundMode::LumaMatte);
    let actual_width = if is_luma { args.width * 2 } else { args.width };

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
        format!("{}x{}", actual_width, args.height),
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

    let file = File::open(&input_path).await?;
    let mut reader = tokio::io::BufReader::new(file).lines();

    let mut emote_ids = HashSet::new();
    let mut image_urls = HashSet::new();
    let mut max_offset_sec: f64 = 0.0;
    let mut base_time_secs: Option<i64> = None;
    let skip_users_set: HashSet<String> = args.skip_users.clone().into_iter().collect();

    while let Some(line) = reader.next_line().await? {
        if let Ok(msg) = serde_json::from_str::<MessageSaved>(&line) {
            if skip_users_set.contains(&msg.sender.username) {
                continue;
            }

            if base_time_secs.is_none() {
                base_time_secs = Some(
                    args.time_zero_ms
                        .map(|t| (t / 1000) as i64)
                        .unwrap_or(msg.created_at_secs),
                );
            }
            let offset_sec = ((msg.created_at_secs - base_time_secs.unwrap()) as f64).max(0.0);
            max_offset_sec = max_offset_sec.max(offset_sec);

            let tokens = tokenise(&msg.content, None);
            for t in &tokens {
                match t {
                    MessageToken::KickEmote { id, .. } => {
                        if let Ok(i) = id.parse::<i32>() {
                            emote_ids.insert(i);
                        }
                    }
                    MessageToken::ProviderEmote(ResolvedEmote { url, .. })
                    | MessageToken::ImageUrl(url) => {
                        image_urls.insert(url.clone());
                    }
                    _ => {}
                }
            }
        }
    }

    emit_progress(5.0, "Hydrating caches...");

    let target_emote_h = ((args.font_size + args.line_spacing as f32) * 0.85).ceil() as u32;

    let emote_cache = Arc::new(EmoteCache::new(
        cache_dir_base.join("emote_cache"),
        args.max_cached_emotes,
        target_emote_h,
        args.quality_preset.clone(),
    ));
    let img_cache = Arc::new(ImageCache::new(
        cache_dir_base.join("image_cache"),
        args.max_cached_emotes,
        target_emote_h * 4,
        args.quality_preset.clone(),
    ));

    emote_cache
        .ensure_cached(&emote_ids.into_iter().collect::<Vec<_>>())
        .await?;
    img_cache
        .ensure_cached(&image_urls.into_iter().collect::<Vec<_>>())
        .await?;

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

    let (loader_tx, loader_rx) = crossbeam_channel::bounded::<(MessageSaved, bool)>(2048);
    let (stamp_tx, stamp_rx) = crossbeam_channel::bounded::<(u32, Arc<ScheduledMessage>)>(512);

    let loader_path = input_path.clone();
    let loader_cancel = Arc::clone(&cancel_flag);
    let group_window = args.group_messages_window_secs as i64;
    let group_enabled = args.group_messages;

    std::thread::spawn(move || {
        let f = std::fs::File::open(loader_path).unwrap();
        let reader = std::io::BufReader::new(f);
        let mut last_user = String::new();
        let mut last_time = -1i64;

        for line in std::io::BufRead::lines(reader).flatten() {
            if loader_cancel.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(msg) = serde_json::from_str::<MessageSaved>(&line) {
                if !skip_users_set.contains(&msg.sender.username) {
                    let is_grouped = group_enabled
                        && msg.sender.username == last_user
                        && (msg.created_at_secs - last_time) <= group_window;

                    last_user = msg.sender.username.clone();
                    last_time = msg.created_at_secs;

                    if loader_tx.send((msg, is_grouped)).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let args_pr = args.clone();
    let emote_cache_pr = Arc::clone(&emote_cache);
    let img_cache_pr = Arc::clone(&img_cache);
    let pr_cancel = Arc::clone(&cancel_flag);
    let base_time_pr = base_time_secs.unwrap_or(0);

    std::thread::spawn(move || {
        let mut batch = Vec::with_capacity(PRE_RENDER_BATCH_SIZE);
        let mut last_assigned_frame = -1i64;

        let flush_batch = |batch_to_flush: Vec<(MessageSaved, bool)>, last_frame: &mut i64| {
            if batch_to_flush.is_empty() {
                return;
            }

            let rendered: Vec<Option<ScheduledMessage>> = RENDER_POOL.install(|| {
                batch_to_flush
                    .into_par_iter()
                    .map(|(msg, is_grouped)| {
                        PRE_RENDER_MEASURE_CACHE.with(|cache_cell| {
                            let mut measure_cache = cache_cell.borrow_mut();

                            let offset_sec = ((msg.created_at_secs - base_time_pr) as f64).max(0.0);
                            let base_frame = (offset_sec * args_pr.fps as f64).round() as i64;
                            let assigned_frame = if base_frame <= *last_frame {
                                *last_frame + 2
                            } else {
                                base_frame
                            };

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
                                &mut measure_cache,
                                is_grouped,
                            ) {
                                Ok((lines, bubble_w, bubble_h, user_color)) => {
                                    Some(ScheduledMessage {
                                        spawn_frame: assigned_frame as u32,
                                        lines,
                                        bubble_w,
                                        bubble_h,
                                        bg_color: Color::from(&args_pr.bubble_color),
                                        user_color,
                                        is_grouped,
                                    })
                                }
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

        while let Ok(msg_tuple) = loader_rx.recv() {
            if pr_cancel.load(Ordering::SeqCst) {
                break;
            }
            batch.push(msg_tuple);
            if batch.len() >= PRE_RENDER_BATCH_SIZE {
                flush_batch(std::mem::take(&mut batch), &mut last_assigned_frame);
            }
        }
        flush_batch(batch, &mut last_assigned_frame);
    });

    let total_frames = ((max_offset_sec * args.fps as f64).round() as u32)
        + (args.message_hold_seconds * args.fps);

    let bg_color = match args.background_mode {
        BackgroundMode::Transparent => Color::TRANSPARENT,
        BackgroundMode::LumaMatte => Color::BLACK,
        BackgroundMode::ChromaKeyGreen => Color::from_argb(255, 0, 255, 0),
        BackgroundMode::CustomColor => Color::from(&args.background_color),
    };

    let info = ImageInfo::new(
        (actual_width, args.height),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let num_bytes = (actual_width * args.height * 4) as usize;
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
            let info_clone = info.clone();

            let current_chunk = std::mem::take(&mut frame_chunk);
            let blocks: Vec<Vec<(u32, Vec<Arc<ScheduledMessage>>)>> =
                current_chunk.chunks(16).map(|c| c.to_vec()).collect();

            let rendered_pixels: Vec<Vec<u8>> = RENDER_POOL.install(|| {
                blocks
                    .into_par_iter()
                    .flat_map(|block| {
                        let mut out = Vec::with_capacity(block.len());
                        let mut prev_pixels: Option<Vec<u8>> = None;
                        let mut last_bubbles_len = 0;

                        for (frame_id, bubbles) in block {
                            let mut pixels = pool.acquire(num_bytes);
                            unsafe {
                                pixels.set_len(num_bytes);
                            }

                            let is_dirty = bubbles.len() != last_bubbles_len
                                || bubbles.iter().any(|b| {
                                    let age =
                                        (frame_id - b.spawn_frame) as f32 / args_block.fps as f32;
                                    let sliding = args_block.anim_slide && age < 0.5;
                                    let fading_in = args_block.anim_fade_in && age < 0.5;
                                    let fading_out = matches!(
                                        args_block.eviction_strategy,
                                        EvictionStrategy::Timed
                                    ) && age
                                        > args_block.message_hold_seconds as f32;
                                    let has_anim = b.lines.iter().any(|l| {
                                        l.tokens.iter().any(|t| match t {
                                            LayoutToken::Emote { data, .. } => {
                                                matches!(**data, EmoteData::Animated { .. })
                                            }
                                            _ => false,
                                        })
                                    });
                                    sliding || fading_in || fading_out || has_anim
                                });

                            last_bubbles_len = bubbles.len();

                            if !is_dirty && prev_pixels.is_some() {
                                pixels.copy_from_slice(prev_pixels.as_ref().unwrap());
                                out.push(pixels);
                                continue;
                            }

                            SKIA_SURFACE.with(|surf_cell| {
                                let mut surf_opt = surf_cell.borrow_mut();

                                if surf_opt.is_none()
                                    || surf_opt.as_ref().unwrap().width() != actual_width
                                    || surf_opt.as_ref().unwrap().height() != args_block.height
                                {
                                    *surf_opt = Some(
                                        surfaces::raster_n32_premul((
                                            actual_width,
                                            args_block.height,
                                        ))
                                        .unwrap(),
                                    );
                                }

                                let surface = surf_opt.as_mut().unwrap();
                                let canvas = surface.canvas();
                                canvas.clear(bg_color);

                                let mut y_cursor = (args_block.height - args_block.padding) as f32;

                                let mut paint_bg = Paint::default();
                                paint_bg.set_anti_alias(true);

                                let mut mask_bg = Paint::default();
                                mask_bg.set_anti_alias(true);
                                mask_bg.set_color_filter(skia_safe::color_filters::blend(
                                    Color::WHITE,
                                    skia_safe::BlendMode::SrcIn,
                                ));

                                let mut text_paint = Paint::default();
                                text_paint.set_anti_alias(true);

                                for bubble in bubbles.iter() {
                                    let age_secs = (frame_id - bubble.spawn_frame) as f32
                                        / args_block.fps as f32;
                                    let mut alpha = 1.0;

                                    if args_block.anim_fade_in {
                                        let intro = 0.5;
                                        if age_secs < intro {
                                            alpha *= age_secs / intro;
                                        }
                                    }
                                    if matches!(
                                        args_block.eviction_strategy,
                                        EvictionStrategy::Timed
                                    ) {
                                        let fade_start = args_block.message_hold_seconds as f32;
                                        if age_secs > fade_start {
                                            alpha *= 1.0
                                                - ((age_secs - fade_start)
                                                    / args_block.message_fade_out_seconds as f32)
                                                    .clamp(0.0, 1.0);
                                        }
                                    }

                                    let byte_alpha = (255.0 * alpha) as u8;
                                    let top = y_cursor - bubble.bubble_h as f32;
                                    let final_x = args_block.padding as f32;

                                    let x_translate = if args_block.anim_slide && age_secs < 0.5 {
                                        let eased = ease_out(age_secs / 0.5);
                                        final_x
                                            + (1.0 - eased) * (args_block.width as f32 - final_x)
                                    } else {
                                        final_x
                                    };

                                    let mask_modes = if is_luma {
                                        vec![false, true]
                                    } else {
                                        vec![false]
                                    };

                                    for is_mask in mask_modes {
                                        let x_ofs = if is_mask {
                                            x_translate + args_block.width as f32
                                        } else {
                                            x_translate
                                        };
                                        canvas.save();
                                        canvas.translate((x_ofs, top));

                                        // Mutate the cached Paint instances directly to avoid clones and inner allocations
                                        let active_bg =
                                            if is_mask { &mut mask_bg } else { &mut paint_bg };
                                        active_bg.set_color(if is_mask {
                                            Color::WHITE
                                        } else {
                                            bubble.bg_color
                                        });
                                        active_bg.set_alpha(byte_alpha);

                                        canvas.draw_round_rect(
                                            skia_safe::Rect::new(
                                                0.0,
                                                0.0,
                                                bubble.bubble_w as f32,
                                                bubble.bubble_h as f32,
                                            ),
                                            args_block.bubble_radius,
                                            args_block.bubble_radius,
                                            &*active_bg,
                                        );

                                        for (li, line) in bubble.lines.iter().enumerate() {
                                            for (ti, token) in line.tokens.iter().enumerate() {
                                                match token {
                                                    LayoutToken::Glyph { blob, x, y, .. } => {
                                                        let is_username = !bubble.is_grouped
                                                            && li == 0
                                                            && ti == 0;

                                                        if is_mask {
                                                            text_paint.set_color(Color::from_argb(
                                                                byte_alpha, 255, 255, 255,
                                                            ));
                                                            canvas.draw_text_blob(
                                                                blob,
                                                                (*x, *y),
                                                                &text_paint,
                                                            );
                                                        } else {
                                                            let mut base_color = if is_username {
                                                                bubble.user_color
                                                            } else {
                                                                Color::from(
                                                                    &args_block.message_color,
                                                                )
                                                            };
                                                            let base_color = base_color.with_a(
                                                                ((base_color.a() as f32 * alpha).min(255.0)) as u8
                                                            );

                                                            if is_username
                                                                && args_block.username_shadow
                                                            {
                                                                text_paint.set_color(
                                                                    Color::from_argb(
                                                                        (180.0 * alpha) as u8,
                                                                        0,
                                                                        0,
                                                                        0,
                                                                    ),
                                                                );
                                                                canvas.draw_text_blob(
                                                                    blob,
                                                                    (*x + 2.0, *y + 2.0),
                                                                    &text_paint,
                                                                );
                                                            }
                                                            if is_username
                                                                && args_block.outline_usernames
                                                            {
                                                                text_paint.set_style(
                                                                    skia_safe::paint::Style::Stroke,
                                                                );
                                                                text_paint.set_stroke_width(
                                                                    args_block
                                                                        .username_outline_width
                                                                        .unwrap_or(1.5),
                                                                );
                                                                text_paint.set_color(
                                                                    Color::from_argb(
                                                                        (200.0 * alpha) as u8,
                                                                        0,
                                                                        0,
                                                                        0,
                                                                    ),
                                                                );
                                                                canvas.draw_text_blob(
                                                                    blob,
                                                                    (*x, *y),
                                                                    &text_paint,
                                                                );
                                                                text_paint.set_style(
                                                                    skia_safe::paint::Style::Fill,
                                                                );
                                                            }

                                                            text_paint.set_color(base_color);
                                                            canvas.draw_text_blob(
                                                                blob,
                                                                (*x, *y),
                                                                &text_paint,
                                                            );
                                                        }
                                                    }
                                                    LayoutToken::Emote { data, x, y } => {
                                                        let ed: &EmoteData = &**data;
                                                        let t_ms = (frame_id as u64 * 1000)
                                                            / args_block.fps as u64;
                                                        if let Some(img) = ed.frame_at(t_ms) {
                                                            canvas.save();
                                                            canvas.translate((*x, *y));
                                                            canvas
                                                                .scale((EMOTE_SCALE, EMOTE_SCALE));
                                                            // Borrow active paint instance immutably to carry the alpha down
                                                            canvas.draw_image(
                                                                img,
                                                                (0, 0),
                                                                Some(&*active_bg),
                                                            );
                                                            canvas.restore();
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        canvas.restore();
                                    }

                                    y_cursor -=
                                        bubble.bubble_h as f32 + args_block.message_spacing as f32;
                                    if y_cursor < 0.0 {
                                        break;
                                    }
                                }

                                surface.read_pixels(
                                    &info_clone,
                                    pixels.as_mut_slice(),
                                    (actual_width * 4) as usize,
                                    (0, 0),
                                );
                            });

                            prev_pixels = Some(pixels.clone());
                            out.push(pixels);
                        }
                        out
                    })
                    .collect()
            });

            for frame_pixels in rendered_pixels {
                let _ = ff_stdin.write_all(&frame_pixels);
                pixel_pool.release(frame_pixels);
            }

            let progress_percent = 10.0 + ((f_idx as f32 / total_frames as f32) * 90.0);
            emit_progress(
                progress_percent,
                &format!("Rendering Video... ({:.1}%)", progress_percent),
            );
        }
    }

    drop(ff_stdin);

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
