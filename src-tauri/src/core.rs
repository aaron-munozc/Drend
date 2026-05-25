pub mod chat;
mod chat_renderer;
pub mod commands;
pub mod download;
pub mod fetcher;
pub mod vod;

pub use commands::download::{get_download_queue, queue_chat_download};
pub use fetcher::analyze_stream_url;
// DownloadManager is consumed by lib.rs; DownloadTask/TaskStatus are public API.
pub use download::manager::TaskManager;
#[allow(unused_imports)]
pub use download::manager::{AppTask, TaskStatus};
