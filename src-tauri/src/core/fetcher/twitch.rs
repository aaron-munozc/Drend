use log::{info, warn};
use regex::Regex;
use tauri_plugin_http::reqwest::Client;
use url::Url;
use urlencoding::encode;

use crate::core::fetcher::traits::MetadataFetcher;
use crate::core::fetcher::types::{ChatMetadata, MediaType, Platform, StreamQuality, UnifiedMetadata};
use crate::core::fetcher::types::kick::KickVideoResponse;
use crate::core::fetcher::types::twitch::{
    GqlClipData, GqlResponse, GqlVideoData, GqlVideoTokenData, TwitchAccessTokenResponse,
};
use crate::error::AppError;
use crate::types::AppResult;

const TWITCH_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
const TWITCH_GQL_URL: &str = "https://gql.twitch.tv/gql";

// --- Internal Routing Enum ---
#[derive(Debug, PartialEq, Eq)]
enum TwitchStream {
    Vod(String),
    Clip(String),
    Invalid,
}

pub struct TwitchFetcher;

#[async_trait::async_trait]
impl MetadataFetcher for TwitchFetcher {
    fn can_handle(&self, url: &str) -> bool {
        url.contains("twitch.tv")
    }

    async fn fetch(&self, client: &Client, url: &str) -> AppResult<Option<UnifiedMetadata>> {
        let stream_type = self.parse_url(url);

        match stream_type {
            TwitchStream::Vod(video_id) => {
                info!("Identified as Twitch VOD. ID: {}", video_id);
                self.fetch_vod(client, &video_id).await
            }
            TwitchStream::Clip(slug) => {
                info!("Identified as Twitch Clip. Slug: {}", slug);
                self.fetch_clip(client, &slug).await
            }
            TwitchStream::Invalid => {
                warn!("URL provided is not a supported Twitch VOD or Clip: {}", url);
                Ok(None)
            }
        }
    }
}

impl TwitchFetcher {
    /// Identifies if the URL is a VOD or a Clip
    fn parse_url(&self, url: &str) -> TwitchStream {
        let parsed = match Url::parse(url) {
            Ok(u) => u,
            Err(_) => return TwitchStream::Invalid,
        };

        let host = parsed.host_str().unwrap_or("");
        let segments: Vec<&str> = parsed
            .path_segments()
            .map(|s| s.filter(|seg| !seg.is_empty()).collect())
            .unwrap_or_default();

        // Check for Clips
        if host.contains("clips.twitch.tv") {
            if let Some(slug) = segments.first() {
                // remove query params if any got stuck
                return TwitchStream::Clip(slug.split('?').next().unwrap().to_string());
            }
        } else if host.contains("twitch.tv") {
            // Check for /clip/ format
            if let Some(pos) = segments.iter().position(|&s| s == "clip") {
                if let Some(slug) = segments.get(pos + 1) {
                    return TwitchStream::Clip(slug.split('?').next().unwrap().to_string());
                }
            }
            // Check for /videos/ format
            if let Some(pos) = segments.iter().position(|&s| s == "videos" || s == "video") {
                if let Some(id) = segments.get(pos + 1) {
                    return TwitchStream::Vod(id.split('?').next().unwrap().to_string());
                }
            }
        }

        TwitchStream::Invalid
    }

    /// Fetches VOD Data, resolves Usher token with legacy fallback, and extracts the M3U8 Master qualities
    /// Fetches VOD Data, resolves Usher token with legacy fallback, and extracts the M3U8 Master qualities
    /// Fetches VOD Data, resolves Usher token, and extracts the M3U8 Master qualities
    async fn fetch_vod(&self, client: &Client, video_id: &str) -> AppResult<Option<UnifiedMetadata>> {
        // 1. Fetch Video Metadata
        let metadata_body = format!(
            r#"{{"query":"query{{video(id:\"{}\"){{title,thumbnailURLs(height:720,width:1280),lengthSeconds,owner{{id,displayName,login}}}}}}","variables":{{}}}}"#,
            video_id
        );

        let meta_resp = client.post(TWITCH_GQL_URL)
            .header("Client-ID", TWITCH_CLIENT_ID)
            .body(metadata_body)
            .send().await?;

        let bytes = meta_resp.bytes().await?;
        let meta_data: GqlResponse<GqlVideoData> = serde_json::from_slice(&bytes)?;

        let video = match meta_data.data.and_then(|d| d.video) {
            Some(v) => v,
            None => return Ok(None),
        };

        // 2. Fetch Access Token via GraphQL (Restored exact payload from your old working code)
        let token_body = format!(
            r#"{{"operationName":"PlaybackAccessToken_Template","query":"query PlaybackAccessToken_Template($login: String!, $isLive: Boolean!, $vodID: ID!, $isVod: Boolean!, $playerType: String!) {{  streamPlaybackAccessToken(channelName: $login, params: {{platform: \"web\", playerBackend: \"mediaplayer\", playerType: $playerType}}) @include(if: $isLive) {{    value    signature    __typename  }}  videoPlaybackAccessToken(id: $vodID, params: {{platform: \"web\", playerBackend: \"mediaplayer\", playerType: $playerType}}) @include(if: $isVod) {{    value    signature    __typename  }} }}","variables":{{"isLive":false,"login":"","isVod":true,"vodID":"{}","playerType":"embed"}}}}"#,
            video_id
        );

        let mut signature: Option<String> = None;
        let mut token_value: Option<String> = None;

        // Sent with ONLY the Client-ID, exactly like the old code.
        let token_resp = client.post(TWITCH_GQL_URL)
            .header("Client-ID", TWITCH_CLIENT_ID)
            .body(token_body)
            .send().await?;

        // Read as text first so we can log it if it fails!
        let token_text = token_resp.text().await?;

        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&token_text) {
            if let Some(tok_obj) = v.pointer("/data/videoPlaybackAccessToken") {
                if tok_obj.is_null() {
                    log::warn!("Twitch returned null for the access token! Is this VOD Sub-only or deleted? Response: {}", token_text);
                } else if let (Some(value), Some(sig)) = (tok_obj.get("value"), tok_obj.get("signature")) {
                    token_value = value.as_str().map(|s| s.to_string());
                    signature = sig.as_str().map(|s| s.to_string());
                }
            } else {
                log::error!("GQL Response did not contain videoPlaybackAccessToken. Raw: {}", token_text);
            }
        } else {
            log::error!("Failed to parse Twitch GQL response as JSON. Raw: {}", token_text);
        }

        // Validate final parsing results explicitly before execution
        let sig = signature.ok_or_else(|| AppError::Http(format!("Failed to retrieve VOD token signature. Check terminal logs! Raw: {}", token_text).into()))?;
        let token = token_value.ok_or_else(|| AppError::Http("Failed to retrieve VOD token string value.".into()))?;

        // 3. Construct Usher URL and Fetch Master Playlist
        let master_url = format!(
            "https://usher.ttvnw.net/vod/{}.m3u8?sig={}&token={}&allow_source=true&allow_audio_only=true&platform=web&player_backend=mediaplayer&include_unavailable=true",
            video_id, encode(&sig), encode(&token)
        );

        let master_playlist = client.get(&master_url).send().await?.text().await?;

        // 4. Parse M3U8 for Stream Qualities
        let mut qualities = Vec::new();
        let mut current_label = "Source".to_string();
        let video_re = Regex::new(r#"VIDEO="([^"]+)""#).unwrap();
        let res_re = Regex::new(r#"RESOLUTION=(\d+x\d+)"#).unwrap();

        for line in master_playlist.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("#EXT-X-STREAM-INF:") {
                if let Some(vid_caps) = video_re.captures(trimmed) {
                    let vid = vid_caps.get(1).unwrap().as_str();
                    current_label = if vid == "chunked" { "Source (Auto)".to_string() } else { vid.to_string() };
                } else if let Some(res_caps) = res_re.captures(trimmed) {
                    current_label = res_caps.get(1).unwrap().as_str().to_string();
                }
            } else if !trimmed.starts_with('#') && !trimmed.is_empty() {
                let download_url = if trimmed.starts_with("http") {
                    trimmed.to_string()
                } else {
                    Url::parse(&master_url).unwrap().join(trimmed).unwrap().to_string()
                };

                qualities.push(StreamQuality {
                    index: qualities.len(),
                    label: current_label.clone(),
                    download_url,
                });
                current_label = "Unknown".to_string();
            }
        }

        let username = video.owner.and_then(|o| o.display_name.or(o.login)).unwrap_or_default();
        let thumbnail = video.thumbnail_urls.and_then(|mut urls| urls.pop());

        let channel_slug = video.owner.as_ref().and_then(|o| o.login.clone()).unwrap_or_default();
        let chat_info = Some(ChatMetadata {
            chat_id: video_id.to_string(),
            channel_slug,
            platform: Platform::Twitch,
        });

        Ok(Some(UnifiedMetadata {
            platform: Platform::Twitch,
            media_type: MediaType::Vod,
            id: video_id.to_string(),
            title: video.title.unwrap_or_else(|| format!("{} VOD", username)),
            username,
            thumbnail_url: thumbnail,
            duration_ms: (video.length_seconds.unwrap_or(0) * 1000) as u64,
            qualities,
            chat_info,
        }))
    }
    /// Fetches Clip Data directly via GraphQL and extracts MP4 direct links
    async fn fetch_clip(&self, client: &Client, slug: &str) -> AppResult<Option<UnifiedMetadata>> {
        let body = format!(
            r#"{{"query":"query($slug: ID!) {{ clip(slug: $slug) {{ id title durationSeconds thumbnailURL broadcaster {{ displayName login }} videoQualities {{ frameRate quality sourceURL }} }} }}","variables":{{"slug":"{}"}}}}"#,
            slug
        );

        let resp = client.post(TWITCH_GQL_URL)
            .header("Client-ID", TWITCH_CLIENT_ID)
            .body(body)
            .send().await?;


        let bytes = resp.bytes().await?;
        let parsed: GqlResponse<GqlClipData> = serde_json::from_slice(&bytes)?;

        let clip = match parsed.data.and_then(|d| d.clip) {
            Some(c) => c,
            None => return Ok(None),
        };

        // Extract the MP4 qualities (usually 1080, 720, 480, 360)
        let mut qualities = Vec::new();
        if let Some(vq_list) = clip.video_qualities {
            for vq in vq_list {
                if let Some(source_url) = vq.source_url {
                    let label = format!("{}p{}", vq.quality.unwrap_or_default(), vq.frame_rate.unwrap_or(30.0) as u32);
                    qualities.push(StreamQuality {
                        index: qualities.len(),
                        label,
                        download_url: source_url,
                    });
                }
            }
        }

        let username = clip.broadcaster.and_then(|b| b.display_name.or(b.login)).unwrap_or_default();

        Ok(Some(UnifiedMetadata {
            platform: Platform::Twitch,
            media_type: MediaType::Clip,
            id: clip.id.unwrap_or_else(|| slug.to_string()),
            title: clip.title.unwrap_or_else(|| format!("Clip by {}", username)),
            username,
            thumbnail_url: clip.thumbnail_url,
            duration_ms: (clip.duration_seconds.unwrap_or(0.0) * 1000.0) as u64,
            qualities, // Clips can have an array of MP4 choices!
        }))
    }
}