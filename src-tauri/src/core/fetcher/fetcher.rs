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

    let is_chat_supported = url.contains("twitch.tv") || url.contains("kick.com");

    let is_live =
        yt_meta.live_status.as_deref() == Some("is_live") || yt_meta.is_live.unwrap_or(false);
    let was_live =
        yt_meta.live_status.as_deref() == Some("was_live") || yt_meta.was_live.unwrap_or(false);
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

    let available_subs = yt_meta
        .subtitles
        .map(|subs| subs.keys().cloned().collect())
        .unwrap_or_default();

    // --- Format Normalization Logic ---
    let formats = yt_meta
        .formats
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| {
            let ext = f.ext.clone().unwrap_or_else(|| "unknown".to_string());

            // Filter out storyboards/manifests (like Twitch's mhtml files)
            if ext == "mhtml" || f.format_id.starts_with("sb") {
                return None;
            }

            let vcodec = f.vcodec.as_deref().unwrap_or("none");
            let acodec = f.acodec.as_deref().unwrap_or("none");

            let has_video = vcodec != "none";
            let has_audio = acodec != "none";

            // Skip completely empty formats
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

            // Example output: "1080p 60fps (mp4) - [V+A]"
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
            })
        })
        .collect();

    let normalized = NormalizedMetadata {
        id: yt_meta.id,
        title: yt_meta
            .title
            .or(yt_meta.fulltitle)
            .unwrap_or_else(|| "Unknown Title".to_string()),
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
        tags: yt_meta.tags.unwrap_or_default(),
        categories: yt_meta.categories.unwrap_or_default(),
        chapters,
        available_subs,
        formats, // Added to struct
        extractor: yt_meta.extractor,
        is_chat_supported,
        original_url: url.clone(),
    };

    let stream_metadata = if is_chat_supported {
        if let Ok(stream) = fetch_stream(client, &url).await {
            Some(stream.into_inner())
        } else {
            None
        }
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
