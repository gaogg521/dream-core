//! Passphrase encryption for backup archives.
//!
//! A backup is a credential. It carries `users.jwt_secret`, and with the
//! provider category it carries every API key the install holds — the archive
//! this was built for was a 17 MB file sitting on a desktop with
//! `containsCredentials: true` in its own manifest. Until now anyone who could
//! read that file had the keys.
//!
//! # What is encrypted, and what deliberately is not
//!
//! The catalog and the copied files are encrypted; `manifest.json` stays in the
//! clear. The manifest holds a timestamp, an app version, which categories were
//! selected and whether credentials are inside — nothing that is worth hiding,
//! and keeping it readable is what lets the app show a person what an archive
//! contains BEFORE asking for the passphrase. Demanding the passphrase to
//! answer "what is this file?" would be security theatre paid for in confusion.
//!
//! # Why Argon2id rather than the key derivation already in this workspace
//!
//! `derive_encryption_key` is a single SHA-256 over a machine-generated secret.
//! That is fine for what it does: the input has full entropy, so iteration
//! count buys nothing. A passphrase is chosen by a person and the archive sits
//! on a disk or a USB stick with no rate limit in front of it, so the cost of a
//! guess is the only defence there is. SHA-256 would make guessing free.
//!
//! # No recovery
//!
//! There is no escrow and no reset: a forgotten passphrase means the archive is
//! unreadable, permanently. That is the point of encrypting it, and the UI says
//! so before the file is written.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

use crate::error::SystemError;

/// Bytes of salt. 16 is the Argon2 recommendation and what its own encoding
/// uses.
const SALT_LEN: usize = 16;
/// AES-GCM standard nonce length.
const NONCE_LEN: usize = 12;

/// Argon2id cost. Chosen to take roughly a quarter of a second on a desktop:
/// long enough that guessing at scale is expensive, short enough that a person
/// unlocking their own backup does not think the app has hung.
const MEMORY_KIB: u32 = 64 * 1024;
const ITERATIONS: u32 = 3;
const PARALLELISM: u32 = 1;

/// Shortest passphrase accepted.
///
/// A slow KDF raises the cost per guess; it cannot rescue a passphrase with
/// almost no guesses in it. The floor is enforced where the archive is written
/// rather than left to the UI, so an archive can never be created below it.
pub const MIN_PASSPHRASE_LEN: usize = 8;

/// How an archive was encrypted, recorded in the (cleartext) manifest.
///
/// Stored rather than assumed so the parameters can change later without
/// stranding archives written today: a reader uses what the file says, not what
/// this build happens to prefer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEncryption {
    /// Cipher name. Only `aes-256-gcm` exists today; a reader refuses anything
    /// else rather than guessing.
    pub cipher: String,
    /// KDF name. Only `argon2id` today.
    pub kdf: String,
    /// Base64 salt for the KDF. Unique per archive.
    pub salt: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    /// Base64 of a fixed marker encrypted with the derived key, so a wrong
    /// passphrase is reported as a wrong passphrase instead of surfacing as a
    /// corrupt-database error several megabytes later.
    pub verifier: String,
}

const CIPHER_NAME: &str = "aes-256-gcm";
const KDF_NAME: &str = "argon2id";
/// Plaintext of the verifier. Its value does not matter; that both sides agree
/// on it does.
const VERIFIER_PLAINTEXT: &[u8] = b"dream-backup-v1";

/// A derived key plus the parameters that produced it.
pub struct ArchiveKey {
    key: [u8; 32],
    encryption: ArchiveEncryption,
}

/// Written by hand rather than derived: a derived `Debug` would print the key
/// bytes, and this type ends up inside `Result`s that tests and error paths
/// format. The parameters are safe to show — they are in the archive's
/// cleartext manifest anyway — and the key never is.
impl std::fmt::Debug for ArchiveKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveKey")
            .field("key", &"<redacted>")
            .field("kdf", &self.encryption.kdf)
            .field("cipher", &self.encryption.cipher)
            .finish()
    }
}

impl ArchiveKey {
    /// Derives a fresh key for a new archive: new salt, current parameters.
    pub fn new(passphrase: &str) -> Result<Self, SystemError> {
        if passphrase.chars().count() < MIN_PASSPHRASE_LEN {
            return Err(SystemError::BadRequest(format!(
                "The backup passphrase must be at least {MIN_PASSPHRASE_LEN} characters."
            )));
        }

        let mut salt = [0u8; SALT_LEN];
        getrandom::getrandom(&mut salt)
            .map_err(|error| SystemError::Internal(format!("Could not generate a salt: {error}")))?;

        let key = derive(passphrase, &salt, MEMORY_KIB, ITERATIONS, PARALLELISM)?;
        let verifier = encrypt_bytes(&key, VERIFIER_PLAINTEXT)?;

        Ok(Self {
            key,
            encryption: ArchiveEncryption {
                cipher: CIPHER_NAME.to_owned(),
                kdf: KDF_NAME.to_owned(),
                salt: BASE64.encode(salt),
                memory_kib: MEMORY_KIB,
                iterations: ITERATIONS,
                parallelism: PARALLELISM,
                verifier: BASE64.encode(verifier),
            },
        })
    }

    /// Re-derives the key of an existing archive and checks the passphrase
    /// against its verifier.
    ///
    /// The check is what turns "wrong passphrase" into those words. Without it
    /// the first sign would be a decryption failure on the catalog, which reads
    /// as a damaged file — and a person who mistyped would go looking for a
    /// corrupted backup instead of retyping.
    pub fn reopen(passphrase: &str, encryption: &ArchiveEncryption) -> Result<Self, SystemError> {
        if encryption.cipher != CIPHER_NAME || encryption.kdf != KDF_NAME {
            return Err(SystemError::BadRequest(format!(
                "This backup uses {} with {}, which this version cannot read.",
                encryption.cipher, encryption.kdf
            )));
        }
        let salt = BASE64
            .decode(&encryption.salt)
            .map_err(|_| SystemError::BadRequest("This backup's encryption header is damaged.".to_owned()))?;

        let key = derive(
            passphrase,
            &salt,
            encryption.memory_kib,
            encryption.iterations,
            encryption.parallelism,
        )?;

        let verifier = BASE64
            .decode(&encryption.verifier)
            .map_err(|_| SystemError::BadRequest("This backup's encryption header is damaged.".to_owned()))?;
        match decrypt_bytes(&key, &verifier) {
            Ok(plain) if plain == VERIFIER_PLAINTEXT => Ok(Self {
                key,
                encryption: encryption.clone(),
            }),
            _ => Err(SystemError::BadRequest(
                "That passphrase does not open this backup.".to_owned(),
            )),
        }
    }

    pub fn encryption(&self) -> &ArchiveEncryption {
        &self.encryption
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, SystemError> {
        encrypt_bytes(&self.key, plaintext)
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, SystemError> {
        decrypt_bytes(&self.key, sealed)
    }
}

fn derive(
    passphrase: &str,
    salt: &[u8],
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
) -> Result<[u8; 32], SystemError> {
    let params = Params::new(memory_kib, iterations, parallelism, Some(32))
        .map_err(|error| SystemError::BadRequest(format!("Unusable encryption parameters: {error}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|error| SystemError::Internal(format!("Could not derive the backup key: {error}")))?;
    Ok(key)
}

/// `nonce || ciphertext`. A fresh nonce per call, which matters: AES-GCM leaks
/// the XOR of two plaintexts if a nonce is ever reused under one key, and an
/// archive seals several payloads with the same key.
fn encrypt_bytes(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, SystemError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|error| SystemError::Internal(format!("Could not initialise the cipher: {error}")))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes)
        .map_err(|error| SystemError::Internal(format!("Could not generate a nonce: {error}")))?;
    let sealed = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext)
        .map_err(|error| SystemError::Internal(format!("Could not encrypt the backup: {error}")))?;

    let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn decrypt_bytes(key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, SystemError> {
    if sealed.len() <= NONCE_LEN {
        return Err(SystemError::BadRequest(
            "This backup is damaged: an encrypted part is too short to be valid.".to_owned(),
        ));
    }
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|error| SystemError::Internal(format!("Could not initialise the cipher: {error}")))?;
    let (nonce, body) = sealed.split_at(NONCE_LEN);
    cipher
        .decrypt(Nonce::from_slice(nonce), body)
        .map_err(|_| SystemError::BadRequest("This backup could not be decrypted; it may be damaged.".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_payload_comes_back_unchanged() {
        let key = ArchiveKey::new("correct horse battery").unwrap();
        let sealed = key.seal(b"catalog bytes").unwrap();
        assert_ne!(sealed, b"catalog bytes", "the payload must not be stored in the clear");
        assert_eq!(key.open(&sealed).unwrap(), b"catalog bytes");
    }

    #[test]
    fn the_right_passphrase_reopens_an_archive() {
        let key = ArchiveKey::new("correct horse battery").unwrap();
        let sealed = key.seal(b"catalog bytes").unwrap();

        let reopened = ArchiveKey::reopen("correct horse battery", key.encryption()).unwrap();
        assert_eq!(reopened.open(&sealed).unwrap(), b"catalog bytes");
    }

    /// The verifier exists so this says "wrong passphrase" rather than failing
    /// later on the catalog, where it would read as a damaged file.
    #[test]
    fn a_wrong_passphrase_is_named_as_such_before_anything_is_read() {
        let key = ArchiveKey::new("correct horse battery").unwrap();
        let error = ArchiveKey::reopen("incorrect horse battery", key.encryption()).unwrap_err();
        assert!(
            matches!(&error, SystemError::BadRequest(message) if message.contains("passphrase")),
            "expected a passphrase error, got {error:?}"
        );
    }

    /// Two archives made with the same passphrase must not share a key: a salt
    /// per archive is what stops one cracked passphrase from being precomputed
    /// against every backup a person ever made.
    #[test]
    fn each_archive_gets_its_own_salt() {
        let first = ArchiveKey::new("correct horse battery").unwrap();
        let second = ArchiveKey::new("correct horse battery").unwrap();
        assert_ne!(first.encryption().salt, second.encryption().salt);
        assert_ne!(first.encryption().verifier, second.encryption().verifier);
    }

    /// Sealing the same bytes twice must not produce the same ciphertext, or a
    /// reused nonce would eventually leak plaintext.
    #[test]
    fn sealing_the_same_bytes_twice_uses_a_fresh_nonce() {
        let key = ArchiveKey::new("correct horse battery").unwrap();
        assert_ne!(key.seal(b"same").unwrap(), key.seal(b"same").unwrap());
    }

    #[test]
    fn a_passphrase_below_the_floor_is_refused() {
        let error = ArchiveKey::new("short").unwrap_err();
        assert!(matches!(error, SystemError::BadRequest(_)));
    }

    #[test]
    fn an_unknown_cipher_is_refused_rather_than_guessed_at() {
        let key = ArchiveKey::new("correct horse battery").unwrap();
        let mut header = key.encryption().clone();
        header.cipher = "rot13".to_owned();
        assert!(ArchiveKey::reopen("correct horse battery", &header).is_err());
    }

    /// Truncation must be rejected, not treated as an empty payload.
    #[test]
    fn a_truncated_payload_is_refused() {
        let key = ArchiveKey::new("correct horse battery").unwrap();
        assert!(key.open(&[0u8; 4]).is_err());
    }
}
