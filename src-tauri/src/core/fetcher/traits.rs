use crate::core::fetcher::types::UnifiedMetadata;
use crate::types::AppResult;
use tauri_plugin_http::reqwest::Client;

#[async_trait::async_trait]
pub trait MetadataFetcher: Send + Sync {
    /// Returns true if this parser can handle the provided URL structure
    fn can_handle(&self, url: &str) -> bool;

    /// Contacts the target platform API and normalizes the return payload
    async fn fetch(&self, client: &Client, url: &str) -> AppResult<Option<UnifiedMetadata>>;
}
