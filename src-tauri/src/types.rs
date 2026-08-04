use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use stream_extractor::StreamMetadata;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Deserialize)]
pub struct YtDlpChapter {
    pub start_time: f64,
    pub end_time: f64,
    pub title: Option<String>,
}

// 1. Raw yt-dlp Format Struct
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
    pub tbr: Option<f64>, // Bitrate
    pub filesize: Option<u64>,
    pub filesize_approx: Option<u64>,
    pub resolution: Option<String>,
}

// 2. Frontend-Facing Format Struct
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedFormat {
    pub format_id: String,
    pub resolution_label: String, // e.g., "1080p", "Audio Only"
    pub fps: Option<f64>,
    pub extension: String,
    pub has_video: bool,
    pub has_audio: bool,
    pub size_bytes: Option<u64>,
    pub bitrate: Option<f64>,
    pub ui_label: String, // Clean label for frontend drop-downs
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    pub start_time: f64,
    pub end_time: f64,
    pub title: String,
}

// 3. Raw yt-dlp Output Struct
#[derive(Debug, Deserialize)]
pub struct YtDlpMetadata {
    pub id: String,
    pub title: Option<String>,
    pub fulltitle: Option<String>,
    pub description: Option<String>,
    pub duration: Option<f64>,

    pub uploader: Option<String>,
    pub uploader_id: Option<String>,
    pub uploader_url: Option<String>,
    pub channel: Option<String>,
    pub channel_id: Option<String>,
    pub channel_url: Option<String>,

    pub thumbnail: Option<String>,
    pub view_count: Option<u64>,
    pub concurrent_view_count: Option<u64>,
    pub like_count: Option<u64>,
    pub comment_count: Option<u64>,

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

    pub formats: Option<Vec<YtDlpFormat>>, // Added formats extraction

    pub webpage_url: Option<String>,
    pub extractor: Option<String>,
}

// 4. Frontend-Facing Metadata Struct
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedMetadata {
    pub id: String,
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

    pub tags: Vec<String>,
    pub categories: Vec<String>,
    pub chapters: Vec<Chapter>,
    pub available_subs: Vec<String>,
    pub formats: Vec<NormalizedFormat>, // Passed down cleanly to frontend

    #[serde(skip_serializing_if = "Option::is_none")]
    pub extractor: Option<String>,
    pub is_chat_supported: bool,
    pub original_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(flatten)]
    pub normalized: NormalizedMetadata,

    #[serde(skip_serializing)]
    pub stream_metadata: Option<StreamMetadata>,
}
