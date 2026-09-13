//! The seam that keeps 文件管理 from being an S3 feature.
//!
//! S3 is what every public cloud and every self-hosted gateway (MinIO, Ceph)
//! speaks, so it was the right first driver — but an enterprise NAS speaks
//! WebDAV/SMB/NFS, and a company that already owns one should not have to
//! stand up an object store to use this page.
//!
//! Two operations is the whole contract, because two is all the console does:
//! prove the credentials reach the storage, and list one level of it. Anything
//! richer (upload, delete, presigned links) belongs to whichever driver grows
//! a real caller, not to a trait written in advance for callers that do not
//! exist.

use async_trait::async_trait;

use crate::error::PlatformError;

/// One object or folder in a listing. Protocol-neutral: an S3 common prefix
/// and a WebDAV collection both arrive here as `is_prefix: true`.
#[derive(Debug, Clone)]
pub struct DriverEntry {
    /// Full key/path, used to descend.
    pub key: String,
    /// Name relative to the listing prefix, for display.
    pub name: String,
    pub is_prefix: bool,
    pub size_bytes: Option<i64>,
    /// Epoch milliseconds.
    pub last_modified: Option<i64>,
}

pub struct DriverListing {
    pub entries: Vec<DriverEntry>,
    /// Continuation token, when the protocol has one. WebDAV does not.
    pub next_token: Option<String>,
}

/// Everything a driver needs, already decrypted. Built per request and
/// dropped with it — the secret does not outlive the call.
pub struct DriverConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub force_path_style: bool,
}

#[async_trait]
pub trait StorageDriver: Send + Sync {
    /// Can we reach the storage with these credentials? The message is shown
    /// to the operator verbatim, so it must name the cause ("AccessDenied",
    /// "401 Unauthorized") rather than restate that something failed.
    async fn probe(&self, cfg: &DriverConfig) -> Result<String, PlatformError>;

    /// One level, rooted at `prefix` (already resolved and validated by the
    /// service — a driver never re-derives scope).
    async fn list(
        &self,
        cfg: &DriverConfig,
        prefix: &str,
        token: Option<&str>,
        limit: i32,
    ) -> Result<DriverListing, PlatformError>;
}

/// Protocols the console offers. A closed set: the UI renders different
/// fields per protocol, and an unknown value would render the wrong form.
pub const PROTOCOLS: [&str; 2] = ["s3", "webdav"];

pub fn validate_protocol(protocol: &str) -> Result<(), PlatformError> {
    if PROTOCOLS.contains(&protocol) {
        return Ok(());
    }
    Err(PlatformError::BadRequest(format!(
        "unsupported storage protocol {protocol}; expected one of {}",
        PROTOCOLS.join(", ")
    )))
}

pub fn driver_for(protocol: &str) -> Result<Box<dyn StorageDriver>, PlatformError> {
    match protocol {
        "s3" => Ok(Box::new(crate::storage_s3::S3Driver)),
        "webdav" => Ok(Box::new(crate::storage_webdav::WebDavDriver)),
        other => Err(PlatformError::BadRequest(format!(
            "unsupported storage protocol {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_implemented_protocols_are_accepted() {
        assert!(validate_protocol("s3").is_ok());
        assert!(validate_protocol("webdav").is_ok());
        // Refused here rather than at driver_for, so a bad value cannot be
        // stored and then fail every time someone opens the page.
        assert!(validate_protocol("smb").is_err());
        assert!(validate_protocol("").is_err());
        assert!(driver_for("s3").is_ok());
        assert!(driver_for("webdav").is_ok());
        assert!(driver_for("nfs").is_err());
    }
}
