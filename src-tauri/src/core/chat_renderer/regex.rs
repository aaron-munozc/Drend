use once_cell::sync::Lazy;
use regex::Regex;

/// Matches custom Kick emote tags in the format: [emote:123:KEKW]
pub static EMOTE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[emote:(?P<id>\d+):(?P<name>[^]]+)]").unwrap());

/// Robust URL matcher mapped to word boundaries.
pub static IMAGE_URL_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:https?://|www\.)[-A-Z0-9+&@#/%?=~_|!:,.;]*[-A-Z0-9+&@#/%=~_|]\.(?:png|jpg|jpeg|gif|webp)(?:[?#][^ ]*)?").unwrap()
});
