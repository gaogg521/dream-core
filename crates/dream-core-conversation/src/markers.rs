//! Escape user-supplied text that mimics server-injected attachment/session markers.
//!
//! Server-generated blocks are appended *after* this pass, so only the user's
//! original message body is sanitized.

use dream_core_common::constants::FILES_MARKER;

/// Injected when routing a message to another conversation (WP3).
pub const SESSIONS_MARKER: &str = "[[DREAM_SESSIONS]]";

/// Injected into the recipient conversation by the session drainer (WP3).
pub const SESSION_MESSAGE_MARKER: &str = "[[DREAM_SESSION_MESSAGE]]";

const MARKER_NEEDLE: &str = "[[DREAM_";
const ZWSP: char = '\u{200B}';

/// Break literal `[[DREAM_*]]` sequences in user text so attachment/session
/// parsers never treat them as trusted server blocks.
pub fn escape_marker_text(content: &str) -> String {
    content.replace(MARKER_NEEDLE, &format!("[[{ZWSP}DREAM_"))
}

pub fn files_marker() -> &'static str {
    FILES_MARKER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_all_dream_marker_prefixes() {
        let raw = format!("a {FILES_MARKER} b {SESSIONS_MARKER} c {SESSION_MESSAGE_MARKER}");
        let escaped = escape_marker_text(&raw);
        assert!(!escaped.contains(MARKER_NEEDLE));
        assert!(escaped.contains("[[\u{200B}DREAM_FILES]]"));
        assert!(escaped.contains("[[\u{200B}DREAM_SESSIONS]]"));
        assert!(escaped.contains("[[\u{200B}DREAM_SESSION_MESSAGE]]"));
    }

    #[test]
    fn normal_brackets_untouched() {
        let raw = "see [[note]] and [[not DREAM at all]]";
        assert_eq!(escape_marker_text(raw), raw);
    }

    #[test]
    fn escaped_files_marker_no_longer_matches_literal_constant() {
        let escaped = escape_marker_text(FILES_MARKER);
        assert_ne!(escaped, FILES_MARKER);
        assert!(!escaped.starts_with("[[DREAM_"));
    }
}
