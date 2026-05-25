use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UnifiedChatMessage {
    pub id: String,
    pub username: String,
    pub color: String,     // Hex color, e.g., "#FF0000"
    pub content: String,   // The actual text message
    pub offset_sec: f64,   // How many seconds into the VOD this happened
    pub timestamp_ms: u64, // Absolute epoch time of the message
}
