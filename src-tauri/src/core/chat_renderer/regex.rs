use once_cell::sync::Lazy;
use regex::Regex;

/// Matches custom Kick emote tags in the format: [emote:123:KEKW]
pub static EMOTE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[emote:(?P<id>\d+):(?P<name>[^]]+)]").unwrap());

/// Robust URL matcher mapped to word boundaries.
///
/// Intentionally anchored at the start with `\b` and at the end requires at
/// least one path character after the extension so bare filenames typed in
/// chat (e.g. "image.png") don't accidentally match. The `(?:[?#][^ ]*)?`
/// tail captures query strings and fragment identifiers that are part of
/// CDN-signed URLs.
pub static IMAGE_URL_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:https?://|www\.)[-A-Z0-9+&@#/%?=~_|!:,.;]*[-A-Z0-9+&@#/%=~_|]\.(?:png|jpg|jpeg|gif|webp)(?:[?#][^ ]*)?").unwrap()
});