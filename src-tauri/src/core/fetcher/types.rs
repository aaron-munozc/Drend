use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::fmt;
use url::Url;

// =========================================================================
// 1. UNIFIED DOMAIN MODELS (The Single Source of Truth for Frontend)
// =========================================================================

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Platform {
    Twitch,
    Kick,
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Platform::Twitch => write!(f, "twitch"),
            Platform::Kick => write!(f, "kick"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMetadata {
    pub chat_id: String,       // Opaque backend handle (Twitch VOD ID or Kick Chatroom ID)
    pub channel_slug: String,  // Needed for certain platform endpoints
    pub platform: Platform,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaType {
    Live,
    Vod,
    Clip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamQuality {
    pub index: usize,
    pub label: String,        // e.g., "1080p60 (Source)"
    pub download_url: String, // Target stream playlist or media file URL
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedMetadata {
    pub platform: Platform,
    pub media_type: MediaType,
    pub id: String,
    pub title: String,
    pub username: String,
    pub thumbnail_url: Option<String>,
    pub duration_ms: u64,
    pub qualities: Vec<StreamQuality>,
    pub chat_info: Option<ChatMetadata>,
}

// =========================================================================
// 2. PRIVATE UTILITIES & HELPERS (Shared across platform scrapers)
// =========================================================================

fn parse_srcset(s: &str) -> Option<String> {
    s.split(',')
        .filter_map(|part| {
            let part = part.trim();
            let mut pieces = part.rsplitn(2, ' ');
            let width_str = pieces.next()?;
            let url = pieces.next()?;
            let width = width_str.trim_end_matches('w').parse::<u32>().ok()?;
            Some((width, url.to_string()))
        })
        .max_by_key(|(w, _)| *w)
        .map(|(_, url)| url)
}

fn deserialize_thumbnail<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let v: Value = Value::deserialize(deserializer)?;
    match v {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() { return Ok(None); }
            if s.contains(' ') && s.contains('w') { Ok(parse_srcset(s)) } else { Ok(Some(s.to_string())) }
        }
        Value::Object(map) => {
            let best_link = map.get("responsive").or_else(|| map.get("srcset"))
                .and_then(|v| v.as_str()).and_then(|s| parse_srcset(s));
            if best_link.is_some() { return Ok(best_link); }
            let fallback = map.get("url").or_else(|| map.get("src"))
                .and_then(|v| v.as_str()).map(|s| s.to_string());
            if fallback.is_some() { return Ok(fallback); }
            let any_url = map.values().filter_map(|v| v.as_str())
                .find(|s| s.starts_with("http")).map(|s| s.to_string());
            Ok(any_url)
        }
        Value::Array(arr) => {
            let found = arr.iter().find_map(|item| match item {
                Value::String(s) if s.starts_with("http") => Some(s.to_string()),
                Value::Object(_) => item.get("url").and_then(|v| v.as_str()).map(|s| s.to_string()),
                _ => None,
            });
            Ok(found)
        }
        _ => Ok(None),
    }
}

fn validate_m3u8<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        Some(url_str) => {
            if let Ok(parsed) = Url::parse(&url_str) {
                let path = parsed.path().to_lowercase();
                if path.ends_with(".m3u8") || path.ends_with(".m3u") {
                    return Ok(Some(url_str));
                }
            }
            Ok(None)
        }
        None => Ok(None),
    }
}

// =========================================================================
// 3. PLATFORM SPECIFIC RAW RESPONSES (Encapsulated Modules)
// =========================================================================

pub mod kick {
    use super::{deserialize_thumbnail, validate_m3u8};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize, Clone, Default)]
    pub struct Chatroom {
        pub id: Option<i64>,
    }

    #[derive(Debug, Deserialize, Serialize, Clone, Default)]
    pub struct User {
        pub username: Option<String>,
        #[serde(alias = "profilepic", alias = "profile_pic", default)]
        pub profile_pic: Option<String>,
        #[serde(default)]
        pub bio: Option<String>,
    }

    #[derive(Debug, Deserialize, Serialize, Clone, Default)]
    pub struct Channel {
        #[serde(rename = "id", alias = "channel_id")]
        pub id: Option<i64>,
        pub slug: Option<String>,
        #[serde(rename = "followersCount", alias = "followers_count", default)]
        pub followers_count: Option<i64>,
        #[serde(default)]
        pub user: Option<User>,
        #[serde(default)]
        pub chatroom: Option<Chatroom>,
        #[serde(default, alias = "playbackUrl")]
        pub playback_url: Option<String>,
    }

    #[derive(Debug, Deserialize, Serialize, Clone)]
    #[serde(untagged)]
    pub enum ChannelField {
        Id(i64),
        Obj(Channel),
    }

    impl Default for ChannelField {
        fn default() -> Self {
            ChannelField::Id(0)
        }
    }

    #[derive(Debug, Deserialize, Serialize, Clone, Default)]
    pub struct Livestream {
        pub id: Option<i64>,
        pub session_title: Option<String>,
        pub start_time: Option<String>,
        pub duration: Option<i64>,
        #[serde(deserialize_with = "deserialize_thumbnail", default)]
        pub thumbnail: Option<String>,
        #[serde(rename = "viewer_count", alias = "viewerCount", default)]
        pub viewer_count: Option<i64>,
        pub is_live: Option<bool>,
        pub channel: Option<ChannelField>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct KickVideoResponse {
        pub uuid: Option<String>,
        pub views: Option<i64>,
        #[serde(deserialize_with = "validate_m3u8")]
        pub source: Option<String>,
        pub playback_url: Option<String>,
        pub livestream: Option<Livestream>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct KickChannelResponse {
        pub id: Option<i64>,
        pub user: Option<User>,
        pub chatroom: Option<Chatroom>,
        pub livestream: Option<Livestream>,
        #[serde(rename = "followersCount", alias = "followers_count")]
        pub followers_count: Option<i64>,
        pub playback_url: Option<String>,
    }
}

pub mod twitch {
    use serde::Deserialize;

    // --- Shared & VOD Types ---
    #[derive(Debug, Deserialize)]
    pub struct GqlOwner {
        pub id: Option<String>,
        #[serde(rename = "displayName")]
        pub display_name: Option<String>,
        pub login: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct GqlVideo {
        pub title: Option<String>,
        #[serde(rename = "thumbnailURLs")]
        pub thumbnail_urls: Option<Vec<String>>,
        #[serde(rename = "lengthSeconds")]
        pub length_seconds: Option<i64>,
        pub owner: Option<GqlOwner>,
    }

    #[derive(Debug, Deserialize)]
    pub struct GqlVideoData {
        pub video: Option<GqlVideo>,
    }

    #[derive(Debug, Deserialize)]
    pub struct TwitchAccessTokenResponse {
        pub token: String,
        pub sig: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct VideoPlaybackAccessToken {
        pub value: String,
        pub signature: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct GqlVideoTokenData {
        #[serde(rename = "videoPlaybackAccessToken")]
        pub video_playback_access_token: Option<VideoPlaybackAccessToken>,
    }

    // --- Clip Types ---
    #[derive(Debug, Deserialize)]
    pub struct GqlClipQuality {
        #[serde(rename = "frameRate")]
        pub frame_rate: Option<f64>,
        pub quality: Option<String>,
        #[serde(rename = "sourceURL")]
        pub source_url: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    pub struct GqlClip {
        pub id: Option<String>,
        pub title: Option<String>,
        #[serde(rename = "durationSeconds")]
        pub duration_seconds: Option<f64>,
        #[serde(rename = "thumbnailURL")]
        pub thumbnail_url: Option<String>,
        pub broadcaster: Option<GqlOwner>,
        #[serde(rename = "videoQualities")]
        pub video_qualities: Option<Vec<GqlClipQuality>>,
    }

    #[derive(Debug, Deserialize)]
    pub struct GqlClipData {
        pub clip: Option<GqlClip>,
    }

    // --- Generic Response Wrapper ---
    #[derive(Debug, Deserialize)]
    pub struct GqlResponse<T> {
        pub data: Option<T>,
    }
}