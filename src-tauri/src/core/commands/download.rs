use tauri::State;
use crate::core::download::manager::{DownloadManager, DownloadTask};
use crate::core::fetcher::types::ChatMetadata;
use crate::types::AppResult;

#[tauri::command]
pub async fn queue_chat_download(
    manager: State<'_, DownloadManager>,
    meta: ChatMetadata,
    title: String,
) -> AppResult<String> {
    let id = meta.chat_id.clone();
    manager.enqueue_chat_download(meta, title);
    Ok(id)
}

#[tauri::command]
pub async fn get_download_queue(manager: State<'_, DownloadManager>) -> AppResult<Vec<DownloadTask>> {
    Ok(manager.get_tasks())
}