use log::{debug, info, warn};
use serde::Deserialize;
use tauri_plugin_http::reqwest::header::{ACCEPT, REFERER, USER_AGENT};
use tauri_plugin_http::reqwest::Client;
use url::Url;

use crate::core::fetcher::traits::MetadataFetcher;
use crate::core::fetcher::types::{ChatMetadata, MediaType, Platform, StreamQuality, UnifiedMetadata};
use crate::core::fetcher::types::kick::{ChannelField, KickVideoResponse};
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
    pub channel_id: u64, // 🟢 We need this for the chat_id
    pub started_at: String, // 🟢 Kept here in case you add it to UnifiedMetadata later
}

#[derive(Debug, Deserialize)]
struct KickClipChannel {
    pub username: String,
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
            [_, prefix, uuid, ..] | [prefix, uuid, ..] if *prefix == "videos" || *prefix == "video" => {
                KickStream::Vod(uuid.to_string())
            }
            // Clip: kick.com/username/clips/clip_id OR kick.com/clips/clip_id
            [_, prefix, clip_id, ..] | [prefix, clip_id, ..] if *prefix == "clips" || *prefix == "clip" => {
                // Remove trailing query params if they got caught in the slug
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
            .header(USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .header(REFERER, "https://kick.com/")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let bytes = resp.bytes().await?;
        let parsed: KickVideoResponse = serde_json::from_slice(&bytes)?;

        let playback_url = parsed.playback_url.or(parsed.source).unwrap_or_default();

        // Safely extract the livestream object
        let livestream_data = parsed.livestream.unwrap_or_default();

        let title = livestream_data.session_title.clone().unwrap_or_else(|| "Kick VOD".to_string());
        let thumbnail = livestream_data.thumbnail.clone();

        // 🟢 FIX 1: Extract timing info to handle the "duration == 0" live edge case
        let is_live = livestream_data.is_live.unwrap_or(false);
        let raw_duration = livestream_data.duration.unwrap_or(0);
        let start_time_str = livestream_data.start_time.clone().unwrap_or_default();

        let duration_ms = if is_live && raw_duration == 0 {
            if let Ok(start_dt) = chrono::DateTime::parse_from_rfc3339(&start_time_str) {
                let elapsed = chrono::Utc::now().signed_duration_since(start_dt.with_timezone(&chrono::Utc));
                elapsed.num_milliseconds().max(0) as u64
            } else {
                0
            }
        } else {
            (raw_duration * 1000) as u64
        };

        let mut username = "Unknown".to_string();
        let mut chat_info = None;

        if let Some(ChannelField::Obj(ch)) = livestream_data.channel {
            username = ch.user.as_ref().and_then(|u| u.username.clone()).unwrap_or_else(|| "Unknown".to_string());

            // 🟢 FIX 2: Use the numeric channel ID for Kick's Chatroom handle
            if let Some(c_id) = ch.id {
                chat_info = Some(ChatMetadata {
                    chat_id: c_id.to_string(),
                    channel_slug: ch.slug.clone().unwrap_or_default(),
                    platform: Platform::Kick,
                });
            }
        }

        Ok(Some(UnifiedMetadata {
            platform: Platform::Kick,
            media_type: MediaType::Vod,
            id: uuid.to_string(),
            title,
            username,
            thumbnail_url: thumbnail,
            duration_ms,
            qualities: vec![
                StreamQuality {
                    index: 0,
                    label: "Source (Auto)".to_string(),
                    download_url: playback_url, // For VODs, this is the master .m3u8
                }
            ],
            chat_info,
        }))
    }

    /// Fetches Clip Data and maps it to UnifiedMetadata
    async fn fetch_clip(&self, client: &Client, clip_id: &str) -> AppResult<Option<UnifiedMetadata>> {
        let api_url = format!("https://kick.com/api/v2/clips/{}", clip_id);

        let resp = client.get(&api_url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let bytes = resp.bytes().await?;
        let parsed: KickClipResponse = serde_json::from_slice(&bytes)?;

        let channel_slug = parsed.clip.channel.username.clone();

        // 🟢 FIX 3: Pass Clip's channel_id as the unique chat handle
        let chat_info = Some(ChatMetadata {
            chat_id: parsed.clip.channel_id.to_string(),
            channel_slug,
            platform: Platform::Kick,
        });

        Ok(Some(UnifiedMetadata {
            platform: Platform::Kick,
            media_type: MediaType::Clip,
            id: parsed.clip.id,
            title: parsed.clip.title,
            username: parsed.clip.channel.username,
            thumbnail_url: parsed.clip.thumbnail_url,
            duration_ms: (parsed.clip.duration * 1000.0) as u64, // Normalized safely to ms
            qualities: vec![
                StreamQuality {
                    index: 0,
                    label: "Source MP4".to_string(),
                    download_url: parsed.clip.video_url,
                }
            ],
            chat_info,
        }))
    }
}