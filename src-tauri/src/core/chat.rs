pub mod kick;
pub mod traits;
pub mod twitch;
pub mod types;

// Re-exported as the public API for this module; used by manager and future callers.
#[allow(unused_imports)]
pub use kick::KickChatDownloader;
#[allow(unused_imports)]
pub use traits::{ChatDownloader, ChatProgressPayload};
#[allow(unused_imports)]
pub use twitch::TwitchChatDownloader;
