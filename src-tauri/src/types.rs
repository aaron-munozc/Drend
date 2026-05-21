use crate::error::AppError;

pub type AppResult<T> = Result<T, AppError>;

pub struct AppState {
    pub client: tauri_plugin_http::reqwest::Client,
    // Future expansion: pub queue: Arc<Mutex<DownloadQueue>>,
}

pub type ClientState<'a> = tauri::State<'a, AppState>;