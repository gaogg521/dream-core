//! Unit tests for the vault download `Content-Disposition` header: ASCII
//! names stay a plain quoted token, non-ASCII names get the RFC 5987
//! `filename*=UTF-8''` form (plus an ASCII fallback) so browsers stop saving
//! mojibake names.

use crate::routes::content_disposition_attachment;

#[test]
fn ascii_name_is_a_plain_quoted_token() {
    assert_eq!(
        content_disposition_attachment("vault-repro-small.txt"),
        "attachment; filename=\"vault-repro-small.txt\""
    );
}

#[test]
fn ascii_name_needs_no_filename_star() {
    // Spaces and parentheses are legal inside a quoted-string token; a pure
    // ASCII name stays the plain form without `filename*`.
    assert_eq!(
        content_disposition_attachment("report 2026 (final).pdf"),
        "attachment; filename=\"report 2026 (final).pdf\""
    );
}

#[test]
fn ascii_dangerous_characters_are_sanitized() {
    assert_eq!(
        content_disposition_attachment("a\"b\\c\rd\ne.txt"),
        "attachment; filename=\"a_b_c_d_e.txt\""
    );
}

#[test]
fn chinese_name_uses_rfc5987_filename_star() {
    let value = content_disposition_attachment("中文-vault.txt");
    // 中文 = E4 B8 AD E6 96 87; the fallback token keeps one `_` per char.
    assert_eq!(
        value,
        "attachment; filename=\"__-vault.txt\"; filename*=UTF-8''%E4%B8%AD%E6%96%87-vault.txt"
    );
}

#[test]
fn emoji_name_encodes_as_utf8_bytes() {
    let value = content_disposition_attachment("报🦋.md");
    assert_eq!(
        value,
        "attachment; filename=\"__.md\"; filename*=UTF-8''%E6%8A%A5%F0%9F%A6%8B.md"
    );
}
