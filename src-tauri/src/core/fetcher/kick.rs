use log::{info, warn};
use serde::Deserialize;
use tauri_plugin_http::reqwest::header::{ACCEPT, REFERER, USER_AGENT};
use tauri_plugin_http::reqwest::Client;
use url::Url;

use crate::core::fetcher::traits::MetadataFetcher;
use crate::core::fetcher::types::kick::{ChannelField, KickVideoResponse};
use crate::core::fetcher::types::{
    ChatMetadata, MediaType, Platform, StreamQuality, UnifiedMetadata,
};
use crate::types::AppResult;

// --- Internal Routing Enum ---
#[derive(Debug, PartialEq, Eq)]
enum KickStream {
    Vod(String),
    Clip(String),
    Other,
}

// --- Clip API Structures (v2) ---
#[derive(Debug, Deserialize)]
struct KickClipResponse {
    pub clip: KickClipData,
}

#[derive(Debug, Deserialize)]
struct KickClipData {
    pub id: String,
    pub title: String,
    pub video_url: String,
    pub duration: f32,
    pub thumbnail_url: Option<String>,
    pub channel: KickClipChannel,
    pub channel_id: u64,
    pub started_at: String,
}

#[derive(Debug, Deserialize)]
struct KickClipChannel {
    pub username: String,
    pub slug: String,
}

// --- The Fetcher ---
pub struct KickFetcher;

#[async_trait::async_trait]
impl MetadataFetcher for KickFetcher {
    fn can_handle(&self, url: &str) -> bool {
        url.contains("kick.com")
    }

    async fn fetch(&self, client: &Client, url: &str) -> AppResult<Option<UnifiedMetadata>> {
        let stream_type = self.parse_url(url);

        match stream_type {
            KickStream::Vod(uuid) => {
                info!("Identified as Kick VOD. UUID: {}", uuid);
                self.fetch_vod(client, &uuid).await
            }
            KickStream::Clip(clip_id) => {
                info!("Identified as Kick Clip. ID: {}", clip_id);
                self.fetch_clip(client, &clip_id).await
            }
            KickStream::Other => {
                warn!("URL provided is not a supported Kick URL format: {}", url);
                Ok(None)
            }
        }
    }
}

impl KickFetcher {
    /// Robust URL parsing ported from your old implementation, upgraded for Clips
    fn parse_url(&self, url: &str) -> KickStream {
        let parsed = match Url::parse(url) {
            Ok(u) => u,
            Err(_) => return KickStream::Other,
        };

        if let Some(host) = parsed.host_str() {
            if !host.contains("kick.com") {
                return KickStream::Other;
            }
        }

        let segments: Vec<&str> = parsed
            .path_segments()
            .map(|s| s.filter(|seg| !seg.is_empty()).collect())
            .unwrap_or_default();

        match segments.as_slice() {
            // VOD: kick.com/username/videos/uuid OR kick.com/videos/uuid
            [_, prefix, uuid, ..] | [prefix, uuid, ..]
                if *prefix == "videos" || *prefix == "video" =>
            {
                KickStream::Vod(uuid.to_string())
            }
            // Clip: kick.com/username/clips/clip_id OR kick.com/clips/clip_id
            [_, prefix, clip_id, ..] | [prefix, clip_id, ..]
                if *prefix == "clips" || *prefix == "clip" =>
            {
                let clean_id = clip_id.split('?').next().unwrap_or(clip_id);
                KickStream::Clip(clean_id.to_string())
            }
            _ => KickStream::Other,
        }
    }

    /// Fetches VOD Data and maps it to UnifiedMetadata
    async fn fetch_vod(&self, client: &Client, uuid: &str) -> AppResult<Option<UnifiedMetadata>> {
        let api_url = format!("https://kick.com/api/v1/video/{}", uuid);

        let resp = client
            .get(&api_url)
            .header(ACCEPT, "application/json")
            .header(
                USER_AGENT,
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .header(REFERER, "https://kick.com/")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let bytes = resp.bytes().await?;
        let parsed: KickVideoResponse = serde_json::from_slice(&bytes)?;

        // Fallback sequence: playback_url first, then source
        let download_url = parsed.playback_url.or(parsed.source).unwrap_or_default();

        let livestream_data = parsed.livestream.unwrap_or_default();
        let title = livestream_data
            .session_title
            .clone()
            .unwrap_or_else(|| "Kick VOD".to_string());
        let thumbnail = livestream_data.thumbnail.clone();

        let is_live = livestream_data.is_live.unwrap_or(false);
        let raw_duration = livestream_data.duration.unwrap_or(0);
        let start_time_str = livestream_data.start_time.clone().unwrap_or_default();

        // Kick's internal VOD durations are already presented in milliseconds
        let duration_ms = if is_live && raw_duration == 0 {
            if let Ok(start_dt) = chrono::DateTime::parse_from_rfc3339(&start_time_str) {
                let elapsed =
                    chrono::Utc::now().signed_duration_since(start_dt.with_timezone(&chrono::Utc));
                elapsed.num_milliseconds().max(0) as u64
            } else {
                0
            }
        } else {
            raw_duration as u64
        };

        let mut username = "Unknown".to_string();
        let mut chat_info = None;

        if let Some(ChannelField::Obj(ch)) = livestream_data.channel {
            username = ch
                .user
                .as_ref()
                .and_then(|u| u.username.clone())
                .unwrap_or_else(|| "Unknown".to_string());

            if let Some(c_id) = ch.id {
                let start_time = chrono::DateTime::parse_from_rfc3339(&start_time_str)
                    .ok()
                    .map(|dt| dt.with_timezone(&chrono::Utc));

                chat_info = Some(ChatMetadata {
                    chat_id: c_id.to_string(),
                    channel_slug: ch.slug.clone().unwrap_or_default(),
                    platform: Platform::Kick,
                    start_time,
                    duration_ms,
                });
            }
        }

        // Parse adaptive qualities directly from the .m3u8 manifest file
        let qualities = self
            .parse_m3u8(client, &download_url, "Source (Auto)")
            .await;

        Ok(Some(UnifiedMetadata {
            platform: Platform::Kick,
            media_type: MediaType::Vod,
            id: uuid.to_string(),
            title,
            username,
            thumbnail_url: thumbnail,
            duration_ms,
            qualities,
            chat_info,
        }))
    }

    /// Fetches Clip Data and maps it to UnifiedMetadata
    async fn fetch_clip(
        &self,
        client: &Client,
        clip_id: &str,
    ) -> AppResult<Option<UnifiedMetadata>> {
        let api_url = format!("https://kick.com/api/v2/clips/{}", clip_id);

        let resp = client
            .get(&api_url)
            .header(ACCEPT, "application/json")
            .header(
                USER_AGENT,
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .send()
            .await?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let bytes = resp.bytes().await?;
        let parsed: KickClipResponse = serde_json::from_slice(&bytes)?;

        let channel_slug = parsed.clip.channel.slug.clone();
        let start_time = chrono::DateTime::parse_from_rfc3339(&parsed.clip.started_at)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc));

        // Clips duration is given as a standard float/integer in seconds (e.g. 21)
        let duration_ms = (parsed.clip.duration * 1000.0) as u64;

        let chat_info = Some(ChatMetadata {
            chat_id: parsed.clip.channel_id.to_string(),
            channel_slug,
            platform: Platform::Kick,
            start_time,
            duration_ms,
        });

        // Some Kick Clip endpoints return an .m3u8 reference rather than an asset .mp4 file.
        // We attempt to dynamically parse it just in case.
        let qualities = self
            .parse_m3u8(client, &parsed.clip.video_url, "Source (M3U8)")
            .await;

        Ok(Some(UnifiedMetadata {
            platform: Platform::Kick,
            media_type: MediaType::Clip,
            id: parsed.clip.id,
            title: parsed.clip.title,
            username: parsed.clip.channel.username,
            thumbnail_url: parsed.clip.thumbnail_url,
            duration_ms,
            qualities,
            chat_info,
        }))
    }

    /// Dynamically parses a master HLS playlist to break down individual quality variants
    async fn parse_m3u8(
        &self,
        client: &Client,
        playlist_url: &str,
        fallback_label: &str,
    ) -> Vec<StreamQuality> {
        let mut qualities = Vec::new();

        // Quick exit if it's not a playlist file
        if !playlist_url.contains(".m3u8") {
            qualities.push(StreamQuality {
                index: 0,
                label: fallback_label.to_string(),
                download_url: playlist_url.to_string(),
            });
            return qualities;
        }

        // Fetch the manifest string content safely
        let resp = match client
            .get(playlist_url)
            .header(
                USER_AGENT,
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => {
                qualities.push(StreamQuality {
                    index: 0,
                    label: fallback_label.to_string(),
                    download_url: playlist_url.to_string(),
                });
                return qualities;
            }
        };

        let text = match resp.text().await {
            Ok(t) => t,
            Err(_) => {
                qualities.push(StreamQuality {
                    index: 0,
                    label: fallback_label.to_string(),
                    download_url: playlist_url.to_string(),
                });
                return qualities;
            }
        };

        let base_url = Url::parse(playlist_url).ok();
        let mut current_resolution = None;

        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("#EXT-X-STREAM-INF:") {
                // Parse out the RESOLUTION parameter value (e.g., RESOLUTION=1920x1080)
                if let Some(pos) = line.find("RESOLUTION=") {
                    let rem = &line[pos + 11..];
                    let end_pos = rem.find(',').unwrap_or(rem.len());
                    current_resolution = Some(rem[..end_pos].trim_matches('"').to_string());
                }
            } else if !line.is_empty() && !line.starts_with('#') {
                // Resolve URLs relative to the main playlist file location if needed
                let variant_url = if let Some(ref base) = base_url {
                    base.join(line)
                        .map(|u| u.to_string())
                        .unwrap_or_else(|_| line.to_string())
                } else {
                    line.to_string()
                };

                // Formats "1920x1080" into "1080p"
                let label = match &current_resolution {
                    Some(res) => {
                        if let Some(p) = res.find('x') {
                            format!("{}p", &res[p + 1..])
                        } else {
                            res.clone()
                        }
                    }
                    None => "Source".to_string(),
                };

                qualities.push(StreamQuality {
                    index: qualities.len(),
                    label,
                    download_url: variant_url,
                });
                current_resolution = None;
            }
        }

        // If the playlist yielded no streaming variants, drop back to safe default string
        if qualities.is_empty() {
            qualities.push(StreamQuality {
                index: 0,
                label: fallback_label.to_string(),
                download_url: playlist_url.to_string(),
            });
        }

        qualities
    }
}
