use crate::error::AppError;
use crate::types::{AppResult, Metadata};
use crate::AppCache;
use stream_extractor::{fetch_stream, StreamClient};
use tauri::State;

#[tauri::command]
pub async fn analyze_stream_url(
    url: String,
    client: State<'_, StreamClient>,
    cache: State<'_, AppCache>,
) -> AppResult<Metadata> {
    let stream = fetch_stream(&client, &url).await?;

    let qualities = stream.get_qualities().await?;

    let metadata = stream.into_inner();

    let mut lock = cache.streams.lock().map_err(|_| {
        AppError::InternalError("Memory protection subsystem error (Lock Poisoned)".into())
    })?;

    let metadata_with_qualities = Metadata {
        stream_metadata: metadata.clone(),
        qualities,
    };

    lock.put(url, metadata_with_qualities.clone());

    Ok(metadata_with_qualities)
}
