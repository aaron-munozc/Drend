use crate::error::AppError;
use serde::Serialize;
use stream_extractor::{StreamMetadata, StreamQuality};

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub stream_metadata: StreamMetadata,
    pub qualities: Vec<StreamQuality>,
}
