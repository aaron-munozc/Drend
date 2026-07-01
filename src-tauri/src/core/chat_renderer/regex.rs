use once_cell::sync::Lazy;
use regex::Regex;

/// Matches custom emote tags in the format: [emote:123:KEKW]
pub static EMOTE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[emote:(?P<id>\d+):(?P<name>[^]]+)]").unwrap());

/// Matches standard web URLs that point directly to common image formats.
/// Modified to allow query strings and anchors without breaking word boundaries.
pub static IMAGE_URL_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bhttps?://[^ ]+\.(?:png|jpg|jpeg|gif|webp)(?:[?#][^ ]*)?").unwrap()
});
