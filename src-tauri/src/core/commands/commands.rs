use crate::core::chat_renderer::RenderVideoArgs;
use crate::core::manager::manager::{FrontendChatOptions, FrontendVodOptions};
use crate::core::manager::AppTask;
use crate::core::TaskManager;
use crate::error::AppError;
use crate::types::{AppResult, Metadata};
use crate::AppCache;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub async fn queue_chat_download(
    manager: State<'_, TaskManager>,
    cache: State<'_, AppCache>,
    id: String,
    url: String,
    options: Option<FrontendChatOptions>,
) -> AppResult<()> {
    let opts = options.unwrap_or_default();
    let cached = fetch_cached_metadata(&cache, &url)?;

    let metadata = cached.stream_metadata.ok_or_else(|| {
        AppError::Generic("Chat manager is not supported for this platform.".into())
    })?;

    manager.enqueue_chat_download(Some(id), metadata, opts);
    Ok(())
}

#[tauri::command]
pub async fn queue_vod_download(
    manager: State<'_, TaskManager>,
    cache: State<'_, AppCache>,
    id: String,
    url: String,
    options: Option<FrontendVodOptions>,
) -> AppResult<()> {
    let opts = options.unwrap_or_default();
    let cached = fetch_cached_metadata(&cache, &url)?;

    manager.enqueue_vod_download(
        Some(id),
        cached.normalized.original_url,
        cached.normalized.title,
        opts,
    );
    Ok(())
}

#[tauri::command]
pub async fn queue_chat_render(
    app_handle: AppHandle,
    manager: State<'_, TaskManager>,
    id: String,
    json_file_path: String,
    options: Option<RenderVideoArgs>,
) -> AppResult<()> {
    let args = options.unwrap_or_default();
    let input_path = PathBuf::from(json_file_path);

    let cache_dir_base = app_handle.path().app_cache_dir().map_err(|e| {
        AppError::Generic(format!("Failed to parse application cache layout: {}", e))
    })?;

    manager.enqueue_chat_render(Some(id), input_path, args, cache_dir_base);
    Ok(())
}

#[tauri::command]
pub async fn get_download_queue(manager: State<'_, TaskManager>) -> AppResult<Vec<AppTask>> {
    Ok(manager.get_tasks())
}

#[tauri::command]
pub async fn cancel_task(
    manager: State<'_, TaskManager>,
    task_id: String,
) -> AppResult<()> {
    manager
        .cancel_task(&task_id)
        .map_err(AppError::Generic)?;

    Ok(())
}

// ==========================================
// HELPERS
// ==========================================

fn fetch_cached_metadata(
    cache: &State<'_, AppCache>,
    target_url: &str,
) -> Result<Metadata, AppError> {
    let mut lock = cache.streams.lock().map_err(|_| {
        AppError::InternalError("Memory protection subsystem error (Lock Poisoned)".into())
    })?;

    lock.get(target_url).cloned().ok_or_else(|| {
        AppError::Generic(
            "Target system index metadata expired or not found. Please analyze the URL again."
                .into(),
        )
    })
}
