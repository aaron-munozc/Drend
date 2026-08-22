use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use stream_extractor::Stream;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Deserialize)]
pub struct YtDlpChapter {
    pub start_time: f64,
    pub end_time: f64,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct YtDlpFormat {
    pub format_id: String,
    pub format_note: Option<String>,
    pub ext: Option<String>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub tbr: Option<f64>, // Total bitrate
    pub vbr: Option<f64>, // Video bitrate
    pub abr: Option<f64>, // Audio bitrate
    pub asr: Option<u32>, // Audio sample rate (Hz)
    pub audio_channels: Option<u8>,
    pub filesize: Option<u64>,
    pub filesize_approx: Option<u64>,
    pub resolution: Option<String>,
    pub url: Option<String>,
    pub manifest_url: Option<String>,
    pub protocol: Option<String>,
    pub language: Option<String>,
    pub dynamic_range: Option<String>,
    pub http_headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedFormat {
    pub format_id: String,
    pub resolution_label: String,
    pub fps: Option<f64>,
    pub extension: String,
    pub has_video: bool,
    pub has_audio: bool,
    pub size_bytes: Option<u64>,
    pub bitrate: Option<f64>,

    // Player context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_range: Option<String>,

    pub ui_label: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub start_time: f64,
    pub end_time: f64,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct YtDlpMetadata {
    pub id: String,
    pub title: Option<String>,
    pub fulltitle: Option<String>,
    pub description: Option<String>,
    pub duration: Option<f64>,

    // Standard Creators
    pub uploader: Option<String>,
    pub uploader_id: Option<String>,
    pub uploader_url: Option<String>,
    pub channel: Option<String>,
    pub channel_id: Option<String>,
    pub channel_url: Option<String>,

    // Music Specific
    pub track: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub release_year: Option<u32>,
    pub genre: Option<String>,

    // Episodic / TV Specific
    pub series: Option<String>,
    pub season_number: Option<u32>,
    pub episode: Option<String>,
    pub episode_number: Option<u32>,

    // Playlist Context
    pub playlist: Option<String>,
    pub playlist_index: Option<u32>,

    pub thumbnail: Option<String>,
    pub view_count: Option<u64>,
    pub concurrent_view_count: Option<u64>,
    pub like_count: Option<u64>,
    pub comment_count: Option<u64>,
    pub repost_count: Option<u64>, // Social media

    pub timestamp: Option<f64>,
    pub upload_date: Option<String>,
    pub live_status: Option<String>,
    pub is_live: Option<bool>,
    pub was_live: Option<bool>,

    pub tags: Option<Vec<String>>,
    pub categories: Option<Vec<String>>,
    pub age_limit: Option<u8>,
    pub chapters: Option<Vec<YtDlpChapter>>,
    pub subtitles: Option<HashMap<String, serde_json::Value>>,
    pub automatic_captions: Option<HashMap<String, serde_json::Value>>,

    pub availability: Option<String>,
    pub formats: Option<Vec<YtDlpFormat>>,
    pub webpage_url: Option<String>,
    pub extractor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedMetadata {
    pub id: String,

    // --- UNIFIED FIELDS FOR FRONTEND ---
    pub display_title: String,
    pub display_creator: String,
    pub media_type: String, // "Music", "Episode", "Live Stream", "Video"

    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_context: Option<String>,

    // --- RAW FIELDS ---
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploader: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploader_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploader_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub like_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment_count: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_date: Option<String>,

    pub is_live: bool,
    pub was_live: bool,
    pub is_upcoming: bool,
    pub age_limit: u8,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,

    pub tags: Vec<String>,
    pub categories: Vec<String>,
    pub chapters: Vec<Chapter>,
    pub available_subs: Vec<String>,
    pub formats: Vec<NormalizedFormat>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub extractor: Option<String>,
    pub is_chat_supported: bool,
    pub original_url: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub webpage_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(flatten)]
    pub normalized: NormalizedMetadata,

    #[serde(skip_serializing)]
    pub stream_metadata: Option<Stream>,
}