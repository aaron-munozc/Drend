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
// Since this wasn't in your old types.rs, we define it here specifically for the Clip fetcher
#[derive(Debug, Deserialize)]
struct KickClipResponse {
    pub clip: KickClipData,
}

#[derive(Debug, Deserialize)]
struct KickClipData {
    pub id: String,
    pub title: String,
    pub video_url: String, // Direct MP4 link
    pub duration: f32,     // Usually in seconds
    pub thumbnail_url: Option<String>,
    pub channel: KickClipChannel,
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
        let mut chat_info = None;

        let playback_url = parsed.playback_url.or(parsed.source).unwrap_or_default();

        let (title, username, duration, thumbnail) = match parsed.livestream {
            Some(ls) => {
                let uname = match &ls.channel {
                    Some(ChannelField::Obj(ch)) => {
                        // Extract chat information if object is fully populated
                        if let (Some(c_id), Some(slug)) = (ch.chatroom.as_ref().and_then(|c| c.id), &ch.slug) {
                            chat_info = Some(ChatMetadata {
                                chat_id: c_id.to_string(),
                                channel_slug: slug.clone(),
                                platform: Platform::Kick,
                            });
                        }
                        ch.user.as_ref().and_then(|u| u.username.clone()).unwrap_or_default()
                    },
                    _ => "Unknown".to_string(),
                };
                (ls.session_title.unwrap_or_default(), uname, ls.duration.unwrap_or(0), ls.thumbnail)
            }
            None => ("Kick VOD".to_string(), "Unknown".to_string(), 0, None),
        };

        Ok(Some(UnifiedMetadata {
            platform: Platform::Kick,
            media_type: MediaType::Vod,
            id: uuid.to_string(),
            title,
            username,
            thumbnail_url: thumbnail,
            duration_ms: (duration * 1000) as u64,
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

        Ok(Some(UnifiedMetadata {
            platform: Platform::Kick,
            media_type: MediaType::Clip,
            id: parsed.clip.id,
            title: parsed.clip.title,
            username: parsed.clip.channel.username,
            thumbnail_url: parsed.clip.thumbnail_url,
            duration_ms: parsed.clip.duration as u64,
            qualities: vec![
                StreamQuality {
                    index: 0,
                    label: "Source MP4".to_string(),
                    download_url: parsed.clip.video_url, // For Clips, this is the direct .mp4 file
                }
            ],
        }))
    }
}