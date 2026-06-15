use serde::Serialize;
use stream_extractor::{StreamMetadata, StreamQuality};
use crate::error::AppError;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, Serialize)]
pub struct Metadata {
    pub stream_metadata: StreamMetadata,
    pub qualities: Vec<StreamQuality>,
}