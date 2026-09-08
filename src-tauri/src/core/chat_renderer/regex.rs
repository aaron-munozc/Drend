use once_cell::sync::Lazy;
use regex::Regex;

// ─────────────────────────────────────────────────────────────────────────────
// Hot-path compile-time regexes
// ─────────────────────────────────────────────────────────────────────────────

/// Matches Kick native emote tags: `[emote:123456:KEKW]`.
///
/// Groups:
///   - `id`   — the numeric emote ID (parsed to i32 at tokenisation time)
///   - `name` — the human-readable emote name (for logging / fallback text)
pub static EMOTE_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[emote:(?P<id>\d+):(?P<name>[^]]+)]").unwrap());

// ─────────────────────────────────────────────────────────────────────────────
// Fast pre-scan guard
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` if `text` might contain a Kick emote tag.
///
/// A single `memchr`-accelerated byte scan. Used to gate `EMOTE_REGEX` so
/// messages with no `[emote:` substring never pay the regex overhead.
#[inline(always)]
pub fn text_may_have_kick_emote(text: &str) -> bool {
    text.contains("[emote:")
}
