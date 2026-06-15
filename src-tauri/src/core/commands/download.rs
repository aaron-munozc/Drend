use crate::core::download::manager::{FrontendChatOptions, FrontendVodOptions};
use crate::core::{AppTask, TaskManager};
use crate::error::AppError;
use crate::types::{AppResult, Metadata};
use crate::AppCache;
use tauri::State;

#[tauri::command]
pub async fn queue_chat_download(
    manager: State<'_, TaskManager>,
    cache: State<'_, AppCache>,
    url: String,
    options: Option<FrontendChatOptions>,
) -> AppResult<String> {
    let opts = options.unwrap_or_default();
    let metadata = fetch_cached_metadata(&cache, &url)?.stream_metadata;

    let task_id = manager.enqueue_chat_download(metadata, opts);
    Ok(task_id)
}

#[tauri::command]
pub async fn queue_vod_download(
    manager: State<'_, TaskManager>,
    cache: State<'_, AppCache>,
    url: String,
    options: Option<FrontendVodOptions>,
) -> AppResult<String> {
    let opts = options.unwrap_or_default();
    let metadata = fetch_cached_metadata(&cache, &url)?.stream_metadata;

    // Title is derived natively in the manager now
    let task_id = manager.enqueue_vod_download(metadata, opts);
    Ok(task_id)
}

#[tauri::command]
pub async fn get_download_queue(manager: State<'_, TaskManager>) -> AppResult<Vec<AppTask>> {
    Ok(manager.get_tasks())
}

// ==========================================
// HELPERS
// ==========================================

fn fetch_cached_metadata(cache: &State<'_, AppCache>, target_url: &str) -> Result<Metadata, AppError> {
    let mut lock = cache
        .streams
        .lock()
        .map_err(|_| AppError::InternalError("Memory protection subsystem error (Lock Poisoned)".into()))?;

    lock.get(target_url)
        .cloned()
        .ok_or_else(|| AppError::Generic("Target system index metadata expired or not found. Please analyze the URL again.".into()))
}