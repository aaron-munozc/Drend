use std::collections::HashSet;
use crate::error::AppError;
use crate::types::{ // Adjust import paths as necessary
                    AppResult, Chapter, Metadata, NormalizedFormat, NormalizedMetadata, YtDlpMetadata,
};
use crate::{tools, AppCache}; // Adjust import paths as necessary
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
            // Some extractors use manifest_url instead of url
            let format_url = f.url.clone().or_else(|| f.manifest_url.clone())?;
            if format_url.is_empty() { return None; }

            let ext = f.ext.clone().unwrap_or_else(|| "unknown".to_string());
            if ext == "mhtml" || f.format_id.starts_with("sb") {
                return None; // Skip storyboards/webpages
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

            // 🚀 UPGRADE: Richer UI Label Construction
            let mut ui_parts = vec![resolution_label.clone()];

            if let Some(fps) = f.fps {
                if fps > 0.0 {
                    ui_parts.push(format!("{}fps", fps.round()));
                }
            }
            if let Some(dr) = &f.dynamic_range {
                if dr != "SDR" {
                    ui_parts.push(dr.clone()); // Adds "HDR10", "DV", etc.
                }
            }
            if let Some(note) = &f.format_note {
                if !note.is_empty() && !note.contains("DASH") {
                    ui_parts.push(format!("({})", note)); // Adds things like "Premium" or "Source"
                }
            }

            let type_badge = match (has_video, has_audio) {
                (true, true) => "V+A",
                (true, false) => "Video Only",
                (false, true) => "Audio Only",
                _ => "Unknown",
            };

            let ui_label = format!("{} [{}] ({})", ui_parts.join(" "), type_badge, ext);

            Some(NormalizedFormat {
                format_id: f.format_id,
                resolution_label,
                fps: f.fps,
                extension: ext,
                has_video,
                has_audio,
                size_bytes: f.filesize.or(f.filesize_approx),
                bitrate: f.tbr.or(f.vbr).or(f.abr),
                protocol: f.protocol,
                language: f.language,
                dynamic_range: f.dynamic_range,
                ui_label,
                url: format_url,
            })
        })
        .collect();

    formats.sort_by(|a, b| {
        let score_a = (a.has_video as u8 * 2) + (a.has_audio as u8);
        let score_b = (b.has_video as u8 * 2) + (b.has_audio as u8);
        score_b.cmp(&score_a)
               .then_with(|| b.resolution_label.cmp(&a.resolution_label))
               .then_with(|| b.fps.unwrap_or(0.0).partial_cmp(&a.fps.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal))
    });

    // --- 🚀 UPGRADE: Cross-Platform Metadata Coalescing ---
    let display_creator = yt_meta.artist.clone()
                                 .or_else(|| yt_meta.channel.clone())
                                 .or_else(|| yt_meta.uploader.clone())
                                 .unwrap_or_else(|| "Unknown Creator".to_string());

    let display_title = yt_meta.track.clone()
                               .or_else(|| yt_meta.episode.clone())
                               .or_else(|| yt_meta.title.clone())
                               .or_else(|| yt_meta.fulltitle.clone())
                               .unwrap_or_else(|| "Unknown Title".to_string());

    let series_context = if let (Some(s_num), Some(e_num)) = (yt_meta.season_number, yt_meta.episode_number) {
        Some(format!("Season {}, Episode {}", s_num, e_num))
    } else if let Some(playlist) = &yt_meta.playlist {
        let index = yt_meta.playlist_index.map(|i| i.to_string()).unwrap_or_else(|| "?".to_string());
        Some(format!("Playlist: {} (#{})", playlist, index))
    } else {
        None
    };

    let media_type = if yt_meta.track.is_some() || yt_meta.artist.is_some() {
        "Music".to_string()
    } else if yt_meta.series.is_some() || yt_meta.episode.is_some() {
        "Episode".to_string()
    } else if is_live {
        "Live Stream".to_string()
    } else {
        "Video".to_string()
    };

    let normalized = NormalizedMetadata {
        id: yt_meta.id,
        // Unified UI Fields
        display_title,
        display_creator,
        media_type,
        series_context,

        // Original standard fields
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
    analyze_url_core(url, &app, &client, &cache).await
}