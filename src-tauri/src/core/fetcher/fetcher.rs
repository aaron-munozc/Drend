use crate::core::fetcher::kick::KickFetcher;
use crate::core::fetcher::traits::MetadataFetcher;
use crate::core::fetcher::twitch::TwitchFetcher;
use crate::core::fetcher::types::UnifiedMetadata;
use crate::types::{AppResult, ClientState};

#[tauri::command]
pub async fn analyze_stream_url(
    client_state: ClientState<'_>,
    url: String,
) -> AppResult<Option<UnifiedMetadata>> {
    let client = &client_state.client;

    // Register your platform processing strategies
    let fetchers: Vec<Box<dyn MetadataFetcher>> =
        vec![Box::new(TwitchFetcher), Box::new(KickFetcher)];

    // Find the first processing pipeline that recognizes the domain layout
    for fetcher in fetchers {
        if fetcher.can_handle(&url) {
            log::info!("Routing url parsing pipeline to target adapter domain strategy");
            return fetcher.fetch(client, &url).await;
        }
    }

    log::warn!(
        "User provided an unsupported streaming address link signature: {}",
        url
    );
    Ok(None)
}
