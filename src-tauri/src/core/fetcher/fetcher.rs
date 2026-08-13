use std::collections::HashSet;
use crate::error::AppError;
use crate::types::{
    AppResult, Chapter, Metadata, NormalizedFormat, NormalizedMetadata, YtDlpMetadata,
};
use crate::{tools, AppCache};
use stream_extractor::{fetch_stream, StreamClient};
use tauri::{AppHandle, State};
use tokio::process::Command;

pub async fn analyze_url_core(
    url: String,
    app: &AppHandle,
    client: &StreamClient,
    cache: &AppCache,
) -> AppResult<Metadata> {
    let ytdlp_path = tools::get_ytdlp_path(app);

    if !ytdlp_path.exists() {
        return Err(AppError::Generic(
            "yt-dlp is not installed. Please install it first.".into(),
        ));
    }

    let output = Command::new(&ytdlp_path)
        .arg("-J")
        .arg("--no-warnings")
        .arg("--no-playlist")
        .arg(&url)
        .output()
        .await
        .map_err(|e| AppError::Generic(format!("Failed to run yt-dlp: {}", e)))?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Generic(format!("yt-dlp error: {}", err_msg)));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let yt_meta: YtDlpMetadata = serde_json::from_str(&json_str)
        .map_err(|e| AppError::Generic(format!("Failed to parse yt-dlp output: {}", e)))?;

    // 🚀 UPGRADE: Smarter chat support check based on the resolved extractor
    let extractor = yt_meta.extractor.as_deref().unwrap_or("").to_lowercase();
    let is_chat_supported = extractor.contains("twitch") || extractor.contains("kick");

    let is_live = yt_meta.live_status.as_deref() == Some("is_live") || yt_meta.is_live.unwrap_or(false);
    let was_live = yt_meta.live_status.as_deref() == Some("was_live") || yt_meta.was_live.unwrap_or(false);
    let is_upcoming = yt_meta.live_status.as_deref() == Some("is_upcoming");

    let chapters = yt_meta
        .chapters
        .unwrap_or_default()
        .into_iter()
        .map(|c| Chapter {
            start_time: c.start_time,
            end_time: c.end_time,
            title: c.title.unwrap_or_else(|| "Chapter".to_string()),
        })
        .collect();

    // 🚀 UPGRADE: Merge manual and automatic captions cleanly
    let mut subs_set = HashSet::new();
    if let Some(subs) = &yt_meta.subtitles {
        subs_set.extend(subs.keys().cloned());
    }
    if let Some(auto_subs) = &yt_meta.automatic_captions {
        subs_set.extend(auto_subs.keys().cloned());
    }
    let mut available_subs: Vec<String> = subs_set.into_iter().collect();
    available_subs.sort();

    // --- Format Normalization Logic ---
    let mut formats: Vec<NormalizedFormat> = yt_meta
        .formats
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| {
            // Skip if there's no actual download/stream URL
            let format_url = f.url?;
            if format_url.is_empty() { return None; }

            let ext = f.ext.clone().unwrap_or_else(|| "unknown".to_string());
            if ext == "mhtml" || f.format_id.starts_with("sb") {
                return None;
            }

            let vcodec = f.vcodec.as_deref().unwrap_or("none");
            let acodec = f.acodec.as_deref().unwrap_or("none");
            let has_video = vcodec != "none";
            let has_audio = acodec != "none";

            if !has_video && !has_audio {
                return None;
            }

            let resolution_label = if let Some(h) = f.height {
                format!("{}p", h)
            } else if has_video {
                f.resolution.clone().unwrap_or_else(|| "Video".to_string())
            } else {
                "Audio Only".to_string()
            };

            let mut ui_parts = vec![resolution_label.clone()];
            if let Some(fps) = f.fps {
                if fps > 0.0 {
                    ui_parts.push(format!("{}fps", fps.round()));
                }
            }

            let type_badge = match (has_video, has_audio) {
                (true, true) => "V+A",
                (true, false) => "Video Only",
                (false, true) => "Audio Only",
                _ => "Unknown",
            };

            let ui_label = format!("{} ({}) - [{}]", ui_parts.join(" "), ext, type_badge);

            Some(NormalizedFormat {
                format_id: f.format_id,
                resolution_label,
                fps: f.fps,
                extension: ext,
                has_video,
                has_audio,
                size_bytes: f.filesize.or(f.filesize_approx),
                bitrate: f.tbr,
                ui_label,
                url: format_url,
            })
        })
        .collect();

    // 🚀 UPGRADE: Sort formats for the frontend (Highest Quality Video+Audio First)
    formats.sort_by(|a, b| {
        let score_a = (a.has_video as u8 * 2) + (a.has_audio as u8);
        let score_b = (b.has_video as u8 * 2) + (b.has_audio as u8);
        score_b.cmp(&score_a) // V+A first, then Video, then Audio
               .then_with(|| b.resolution_label.cmp(&a.resolution_label))
               .then_with(|| b.fps.unwrap_or(0.0).partial_cmp(&a.fps.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal))
    });

    let normalized = NormalizedMetadata {
        id: yt_meta.id,
        title: yt_meta.title.or(yt_meta.fulltitle).unwrap_or_else(|| "Unknown Title".to_string()),
        description: yt_meta.description,
        duration: yt_meta.duration,
        uploader: yt_meta.uploader.or(yt_meta.channel),
        uploader_id: yt_meta.uploader_id.or(yt_meta.channel_id),
        uploader_url: yt_meta.uploader_url.or(yt_meta.channel_url),
        thumbnail: yt_meta.thumbnail,
        view_count: yt_meta.view_count.or(yt_meta.concurrent_view_count),
        like_count: yt_meta.like_count,
        comment_count: yt_meta.comment_count,
        timestamp: yt_meta.timestamp,
        upload_date: yt_meta.upload_date,
        is_live,
        was_live,
        is_upcoming,
        age_limit: yt_meta.age_limit.unwrap_or(0),
        availability: yt_meta.availability,
        tags: yt_meta.tags.unwrap_or_default(),
        categories: yt_meta.categories.unwrap_or_default(),
        chapters,
        available_subs,
        formats,
        extractor: yt_meta.extractor,
        is_chat_supported,
        original_url: url.clone(),
        webpage_url: yt_meta.webpage_url,
    };

    let stream_metadata = if is_chat_supported {
        fetch_stream(client, &url).await.ok()
    } else {
        None
    };

    let mut lock = cache.streams.lock().map_err(|_| {
        AppError::InternalError("Memory protection subsystem error (Lock Poisoned)".into())
    })?;

    let final_metadata = Metadata {
        normalized,
        stream_metadata,
    };

    lock.put(url, final_metadata.clone());
    Ok(final_metadata)
}
#[tauri::command]
pub async fn analyze_url(
    url: String,
    app: AppHandle,
    client: State<'_, StreamClient>,
    cache: State<'_, AppCache>,
) -> AppResult<Metadata> {
    // Pass it down to the core logic
    analyze_url_core(url, &app, &client, &cache).await
}
