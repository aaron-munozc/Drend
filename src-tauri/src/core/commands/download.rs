use crate::core::chat_renderer::RenderVideoArgs;
use crate::core::download::manager::DownloadOptions;
use crate::core::fetcher::types::ChatMetadata;
use crate::core::{AppTask, TaskManager};
use crate::types::AppResult;
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub async fn queue_chat_download(
    manager: State<'_, TaskManager>,
    meta: ChatMetadata,
    title: String,
    options: Option<DownloadOptions>,
) -> AppResult<String> {
    let opts = options.unwrap_or_default();
    let task_id = manager.enqueue_chat_download(meta, title, opts);
    Ok(task_id)
}

#[tauri::command]
pub async fn queue_vod_download(
    manager: State<'_, TaskManager>,
    m3u8_url: String,
    video_id: String,
    title: String,
    options: Option<DownloadOptions>,
) -> AppResult<String> {
    let opts = options.unwrap_or_default();
    let task_id = manager.enqueue_vod_download(m3u8_url, video_id, title, opts);
    Ok(task_id)
}

// --- NEW COMMAND ---
#[tauri::command]
pub async fn queue_chat_render(
    manager: State<'_, TaskManager>,
    input_path: String,
    title: String,
    args: RenderVideoArgs,
) -> AppResult<String> {
    let path = PathBuf::from(input_path);
    let task_id = manager.enqueue_chat_render(path, title, args);
    Ok(task_id)
}

#[tauri::command]
pub async fn get_download_queue(manager: State<'_, TaskManager>) -> AppResult<Vec<AppTask>> {
    Ok(manager.get_tasks())
}
