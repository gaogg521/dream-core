use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

const NONCE_SIZE: usize = 12;
const KEY_SIZE: usize = 32;

/// Crypto helper error independent of HTTP/API boundaries.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("AES-256 key must be exactly {expected} bytes, got {actual}")]
    InvalidKeySize { expected: usize, actual: usize },

    #[error("Failed to create cipher: {0}")]
    CipherInit(String),

    #[error("RNG failure: {0}")]
    Random(String),

    #[error("Encryption failed: {0}")]
    Encryption(String),

    #[error("Invalid base64: {0}")]
    InvalidBase64(String),

    #[error("Ciphertext too short")]
    CiphertextTooShort,

    #[error("Decryption failed: invalid key or corrupted data")]
    DecryptionFailed,

    #[error("Invalid UTF-8 in decrypted data: {0}")]
    InvalidUtf8(String),
}

impl CryptoError {
    /// Returns true for caller/data problems that API boundaries should map to 400.
    pub fn is_bad_request(&self) -> bool {
        matches!(
            self,
            Self::InvalidKeySize { .. } | Self::InvalidBase64(_) | Self::CiphertextTooShort | Self::DecryptionFailed
        )
    }
}

/// Encrypt a string value using AES-256-GCM.
///
/// The key must be exactly 32 bytes. Output is base64-encoded (nonce + ciphertext + tag).
pub fn encrypt_string(plaintext: &str, key: &[u8]) -> Result<String, CryptoError> {
    validate_key_size(key)?;

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| CryptoError::CipherInit(e.to_string()))?;

    let mut nonce_bytes = [0u8; NONCE_SIZE];
    getrandom::getrandom(&mut nonce_bytes).map_err(|e| CryptoError::Random(e.to_string()))?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| CryptoError::Encryption(e.to_string()))?;

    let mut combined = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&ciphertext);

    Ok(BASE64.encode(combined))
}

/// Decrypt an AES-256-GCM encrypted string.
///
/// The key must be exactly 32 bytes. Input is base64-encoded (nonce + ciphertext + tag).
pub fn decrypt_string(ciphertext: &str, key: &[u8]) -> Result<String, CryptoError> {
    validate_key_size(key)?;

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| CryptoError::CipherInit(e.to_string()))?;

    let combined = BASE64
        .decode(ciphertext)
        .map_err(|e| CryptoError::InvalidBase64(e.to_string()))?;

    if combined.len() < NONCE_SIZE {
        return Err(CryptoError::CiphertextTooShort);
    }

    let (nonce_bytes, encrypted) = combined.split_at(NONCE_SIZE);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, encrypted)
        .map_err(|_| CryptoError::DecryptionFailed)?;

    String::from_utf8(plaintext).map_err(|e| CryptoError::InvalidUtf8(e.to_string()))
}

fn validate_key_size(key: &[u8]) -> Result<(), CryptoError> {
    if key.len() != KEY_SIZE {
        return Err(CryptoError::InvalidKeySize {
            expected: KEY_SIZE,
            actual: key.len(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Versioned field envelope
// ---------------------------------------------------------------------------
//
// `encrypt_string`/`decrypt_string` above assume the caller always knows
// whether a stored value is encrypted. That's true for columns that were
// encrypted from the day they were introduced (e.g. `providers.api_key`), but
// it breaks down for a column that started out storing plaintext and is being
// migrated to encryption-at-rest later: an in-place data migration needs to
// tell "already encrypted" apart from "still legacy plaintext" for the same
// column, and it needs to do so idempotently (safe to re-run after a crash
// mid-migration without double-encrypting a row and turning it into
// unrecoverable garbage on the next decrypt).
//
// `encrypt_field`/`decrypt_field` solve this with an explicit version prefix:
// the ciphertext this module produces is base64, which never happens to start
// with `ENCRYPTED_FIELD_PREFIX` (`:` is not in the base64 alphabet), so the
// prefix is an unambiguous, self-describing marker rather than a heuristic.

/// Prefix marking a value as produced by [`encrypt_field`]. Anything stored
/// without this prefix is legacy plaintext that predates encryption of that
/// field.
pub const ENCRYPTED_FIELD_PREFIX: &str = "encv1:";

/// Returns true if `value` carries the [`encrypt_field`] envelope prefix.
///
/// Used by data migrations to skip rows that are already encrypted, making
/// the migration idempotent — safe to re-run after an interrupted pass.
pub fn is_encrypted_field(value: &str) -> bool {
    value.starts_with(ENCRYPTED_FIELD_PREFIX)
}

/// Encrypt a field for storage, tagging the result with [`ENCRYPTED_FIELD_PREFIX`]
/// so a later read (or an idempotent migration) can tell it apart from legacy
/// plaintext written before this field was covered by encryption.
pub fn encrypt_field(plaintext: &str, key: &[u8]) -> Result<String, CryptoError> {
    let encrypted = encrypt_string(plaintext, key)?;
    Ok(format!("{ENCRYPTED_FIELD_PREFIX}{encrypted}"))
}

/// Decrypt a field written by [`encrypt_field`].
///
/// If `value` does not carry the envelope prefix, it is treated as
/// not-yet-migrated legacy plaintext and returned unchanged rather than
/// erroring — this keeps reads safe even if the one-time migration that
/// encrypts historical rows (see `dream_core_db::encrypt_legacy_plaintext`)
/// has not run yet, or a row was written directly outside the app.
pub fn decrypt_field(value: &str, key: &[u8]) -> Result<String, CryptoError> {
    match value.strip_prefix(ENCRYPTED_FIELD_PREFIX) {
        Some(ciphertext) => decrypt_string(ciphertext, key),
        None => Ok(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        [0x42; 32]
    }

    #[test]
    fn test_roundtrip() {
        let key = test_key();
        let encrypted = encrypt_string("hello", &key).unwrap();
        let decrypted = decrypt_string(&encrypted, &key).unwrap();
        assert_eq!(decrypted, "hello");
    }

    #[test]
    fn test_empty_string() {
        let key = test_key();
        let encrypted = encrypt_string("", &key).unwrap();
        let decrypted = decrypt_string(&encrypted, &key).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_unicode() {
        let key = test_key();
        let encrypted = encrypt_string("你好世界", &key).unwrap();
        let decrypted = decrypt_string(&encrypted, &key).unwrap();
        assert_eq!(decrypted, "你好世界");
    }

    #[test]
    fn test_wrong_key_fails() {
        let key = test_key();
        let encrypted = encrypt_string("hello", &key).unwrap();
        let wrong_key = [0x99; 32];
        assert!(matches!(
            decrypt_string(&encrypted, &wrong_key),
            Err(CryptoError::DecryptionFailed)
        ));
    }

    #[test]
    fn test_nonce_randomness() {
        let key = test_key();
        let enc1 = encrypt_string("hello", &key).unwrap();
        let enc2 = encrypt_string("hello", &key).unwrap();
        assert_ne!(enc1, enc2);
    }

    #[test]
    fn test_invalid_key_size() {
        let short_key = [0u8; 16];
        assert!(matches!(
            encrypt_string("hello", &short_key),
            Err(CryptoError::InvalidKeySize {
                expected: KEY_SIZE,
                actual: 16
            })
        ));
        assert!(matches!(
            decrypt_string("dGVzdA==", &short_key),
            Err(CryptoError::InvalidKeySize {
                expected: KEY_SIZE,
                actual: 16
            })
        ));
    }

    #[test]
    fn test_invalid_base64() {
        let key = test_key();
        assert!(matches!(
            decrypt_string("not-valid-base64!!!", &key),
            Err(CryptoError::InvalidBase64(_))
        ));
    }

    #[test]
    fn test_ciphertext_too_short() {
        let key = test_key();
        // Base64 of less than 12 bytes
        let short = BASE64.encode([0u8; 5]);
        assert!(matches!(
            decrypt_string(&short, &key),
            Err(CryptoError::CiphertextTooShort)
        ));
    }

    // -- Versioned field envelope --------------------------------------------

    #[test]
    fn field_roundtrip() {
        let key = test_key();
        let encrypted = encrypt_field("sk-super-secret", &key).unwrap();
        assert!(is_encrypted_field(&encrypted));
        assert!(encrypted.starts_with(ENCRYPTED_FIELD_PREFIX));
        let decrypted = decrypt_field(&encrypted, &key).unwrap();
        assert_eq!(decrypted, "sk-super-secret");
    }

    #[test]
    fn field_empty_string_roundtrip() {
        let key = test_key();
        let encrypted = encrypt_field("", &key).unwrap();
        assert_eq!(decrypt_field(&encrypted, &key).unwrap(), "");
    }

    #[test]
    fn decrypt_field_passes_through_legacy_plaintext_unchanged() {
        let key = test_key();
        // A pre-migration value with no envelope prefix — e.g. a raw JSON blob
        // or an opaque OAuth token written before this field was encrypted.
        let legacy = r#"{"command":"npx","args":["-y","server"],"env":{"TOKEN":"abc123"}}"#;
        assert!(!is_encrypted_field(legacy));
        assert_eq!(decrypt_field(legacy, &key).unwrap(), legacy);
    }

    #[test]
    fn is_encrypted_field_distinguishes_prefix() {
        assert!(!is_encrypted_field("plain value"));
        assert!(!is_encrypted_field(""));
        assert!(is_encrypted_field("encv1:AAAA"));
    }

    #[test]
    fn encrypt_field_is_not_idempotent_by_itself_callers_must_check_first() {
        // encrypt_field always wraps again — it is the caller's job (via
        // is_encrypted_field) to avoid double-encrypting an already-encrypted
        // value. This test documents that double-wrapping is technically
        // reversible (decrypt_field can be called twice) but callers should
        // never rely on it; the migration is expected to check first.
        let key = test_key();
        let once = encrypt_field("secret", &key).unwrap();
        let twice = encrypt_field(&once, &key).unwrap();
        assert_ne!(once, twice);
        let back_once = decrypt_field(&twice, &key).unwrap();
        assert_eq!(back_once, once);
        let back_twice = decrypt_field(&back_once, &key).unwrap();
        assert_eq!(back_twice, "secret");
    }

    #[test]
    fn decrypt_field_wrong_key_fails_for_actually_encrypted_value() {
        let key = test_key();
        let wrong_key = [0x99u8; 32];
        let encrypted = encrypt_field("secret", &key).unwrap();
        assert!(matches!(
            decrypt_field(&encrypted, &wrong_key),
            Err(CryptoError::DecryptionFailed)
        ));
    }
}
