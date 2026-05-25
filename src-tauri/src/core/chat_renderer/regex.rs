use once_cell::sync::Lazy;
use regex::Regex;

/// Matches custom emote tags in the format: [emote:123:KEKW]
///
/// ### Structure breakdown:
/// `\[emote:`       - Literal start of the tag.
/// `(?P<id>\d+)`    - Capture group 'id': Matches one or more digits.
/// `:`              - Literal separator.
/// `(?P<name>[^]]+)`- Capture group 'name': Matches everything until the closing ']'.
/// `]`              - Literal end of the tag.
pub static EMOTE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[emote:(?P<id>\d+):(?P<name>[^]]+)]").unwrap());

/// Matches standard web URLs that point directly to common image formats.
///
/// ### Structure breakdown:
/// `(?i)`           - Case-insensitive flag (matches .PNG as well as .png).
/// `\bhttps?://`    - Matches 'http://' or 'https://' starting at a word boundary.
/// `[^ ]+`          - Matches one or more non-space characters (the URL body).
/// `\.`             - A literal dot before the extension.
/// `(?:...)`        - A non-capturing group for the allowed file extensions.
/// `\b`             - Ensures the URL ends at a word boundary.
pub static IMAGE_URL_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bhttps?://[^ ]+\.(?:png|jpg|jpeg|gif|webp)\b").unwrap());
