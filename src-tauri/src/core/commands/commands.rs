use crate::core::chat_renderer::RenderVideoArgs;

use crate::core::{AppTask, TaskManager};
use crate::error::AppError;
use crate::types::{AppResult, Metadata};
use crate::AppCache;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};
use crate::core::manager::manager::{BatchRenderItem, FrontendChatOptions, FrontendVodOptions, QueueSettings};

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
        AppError::Generic("Chat downloading is not supported for this platform.".into())
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
    let cache_dir_base = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| AppError::Generic(format!("Failed to resolve cache dir: {}", e)))?;
    manager.enqueue_chat_render(Some(id), input_path, args, cache_dir_base);
    Ok(())
}

#[tauri::command]
pub async fn queue_batch_chat_render(
    app_handle: AppHandle,
    manager: State<'_, TaskManager>,
    items: Vec<BatchRenderItem>,
) -> AppResult<Vec<String>> {
    let cache_dir_base = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| AppError::Generic(format!("Failed to resolve cache dir: {}", e)))?;

    let resolved: Vec<(String, PathBuf, RenderVideoArgs, PathBuf)> = items
        .into_iter()
        .map(|item| {
            (
                item.id,
                PathBuf::from(item.json_file_path),
                item.options,
                cache_dir_base.clone(),
            )
        })
        .collect();

    let ids = manager.enqueue_batch_chat_render(resolved);
    Ok(ids)
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
    manager.cancel_task(&task_id).map_err(AppError::Generic)?;
    Ok(())
}

#[tauri::command]
pub async fn get_queue_settings(
    manager: State<'_, TaskManager>,
) -> AppResult<QueueSettings> {
    Ok(manager.get_settings())
}

#[tauri::command]
pub async fn update_queue_settings(
    manager: State<'_, TaskManager>,
    settings: QueueSettings,
) -> AppResult<()> {
    manager.apply_settings(settings)?;
    Ok(())
}

fn fetch_cached_metadata(
    cache: &State<'_, AppCache>,
    target_url: &str,
) -> Result<Metadata, AppError> {
    let mut lock = cache.streams.lock().map_err(|_| {
        AppError::InternalError("Memory protection subsystem error (Lock Poisoned)".into())
    })?;
    lock.get(target_url).cloned().ok_or_else(|| {
        AppError::Generic("Metadata expired or not found. Please analyze the URL again.".into())
    })
}