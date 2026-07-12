use serde::Serialize;
use thiserror::Error;

/// Combined Error type.
/// The serde attributes here replace the need for a separate ErrorKind enum.
#[allow(dead_code)]
#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "camelCase")]
pub enum AppError {
    #[error("HTTP request failed: {0}")]
    Http(String),

    #[error("Invalid response from API: {0}")]
    Api(String),

    #[error("Playlist parsing failed: {0}")]
    PlaylistParse(String),

    #[error("Segment manager failed: {0}")]
    SegmentDownload(String),

    #[error("Segment write failed: {0}")]
    SegmentWrite(String),

    #[error("Temporary directory failed: {0}")]
    TempDir(String),

    #[error("Emit event failed: {0}")]
    Emit(String),

    #[error("Invalid quality index: {0}")]
    InvalidQualityIndex(usize),

    #[error("JSON error: {0}")]
    Json(
        #[from]
        #[serde(serialize_with = "to_s")]
        serde_json::Error,
    ),

    #[error("Parquet deserialization failed: {0}")]
    Parquet(String),

    #[error("CSV error: {0}")]
    Csv(String),

    #[error("Time parsing failed: {0}")]
    TimeParse(String),

    #[error("Rate limited or blocked by API")]
    RateLimited,

    #[error("File I/O error: {0}")]
    FileIo(String),

    // Handles both rusqlite and r2d2 by using manual map or multiple froms
    #[error("Database error: {0}")]
    Db(String),

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Cancel request")]
    Cancelled,

    #[error("Store error: {0}")]
    Store(String),

    #[error("Polars error: {0}")]
    Polars(String),

    #[error("Sound error: {0}")]
    Sound(String),

    #[error("Ffmpeg error: {0}")]
    Ffmpeg(String),

    #[error("WAV error: {0}")]
    Wav(String),

    #[error("Skia error: {0}")]
    Skia(String),

    #[error("Emote cache error: {0}")]
    EmoteCache(String),

    #[error("Stream extractor error: {0}")]
    StreamExtractor(
        #[from]
        #[serde(serialize_with = "to_s")]
        stream_extractor::Error,
    ),

    #[error("Image error: {0}")]
    Image(
        #[from]
        #[serde(serialize_with = "to_s")]
        image::ImageError,
    ),

    #[error("Error: {0}")]
    Generic(String),
}

impl From<tauri_plugin_http::reqwest::Error> for AppError {
    fn from(e: tauri_plugin_http::reqwest::Error) -> Self {
        Self::Http(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::FileIo(e.to_string())
    }
}

fn to_s<S, T>(err: &T, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: std::fmt::Display,
{
    s.serialize_str(&err.to_string())
}

#[allow(dead_code)]
pub trait Contextualize<T> {
    fn with_context<F>(self, f: F) -> Result<T, AppError>
    where
        F: FnOnce(String) -> AppError;
}

impl<T, E: std::fmt::Display> Contextualize<T> for Result<T, E> {
    fn with_context<F>(self, f: F) -> Result<T, AppError>
    where
        F: FnOnce(String) -> AppError,
    {
        self.map_err(|e| f(e.to_string()))
    }
}
