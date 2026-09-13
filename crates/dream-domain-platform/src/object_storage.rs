//! Enterprise 文件管理: S3-compatible object storage the tenant registers,
//! tests and browses from the console.
//!
//! Deliberately separate from the personal file vault. That one stores member
//! uploads on the local filesystem under `<data_dir>/file-vault` and is a
//! per-member quota feature; this one is the organisation's *external* buckets
//! — the bundled MinIO, or any S3 endpoint — which an admin points the platform
//! at and inspects.
//!
//! Two rules the whole module is built around:
//!
//! 1. **The secret access key is write-only.** Stored encrypted with the
//!    service key (the same helper the container-registry secret uses), never
//!    selected into a DTO, never logged. Callers see `hasSecret: bool`. An
//!    update that omits the field keeps the stored one, so an edit form that
//!    correctly never renders the secret cannot wipe it.
//! 2. **Listing is confined to the configured prefix.** A config may pin a
//!    `prefix`; every listing is rooted there and a caller-supplied sub-prefix
//!    is appended, never substituted. Otherwise "browse the bucket" would let
//!    an admin page read objects the configuration was scoped away from.

use serde::{Deserialize, Serialize};

use dream_core_common::{decrypt_string, encrypt_string, generate_prefixed_id, now_ms};
use dream_core_db::db_params;

use crate::error::PlatformError;
use crate::service::PlatformService;
use crate::storage_driver::{DriverConfig, driver_for, validate_protocol};

/// One registered bucket, as the console renders it. No credential material.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectStorageConfigDto {
    pub id: String,
    /// `'s3' | 'webdav'` — which driver talks to this storage.
    pub protocol: String,
    /// Operator-facing handle, unique per tenant (the reference product's 配置 Key).
    pub config_key: String,
    pub bucket: String,
    pub endpoint: String,
    pub region: String,
    pub prefix: Option<String>,
    pub access_key_id: Option<String>,
    /// Whether a secret is stored — never the secret.
    pub has_secret: bool,
    pub force_path_style: bool,
    pub is_default: bool,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Create/update body. Every field optional so one shape serves both; on
/// create the required ones are checked explicitly.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectStorageConfigInput {
    pub config_key: Option<String>,
    pub protocol: Option<String>,
    pub bucket: Option<String>,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub prefix: Option<String>,
    pub access_key_id: Option<String>,
    /// Write-only. `None` keeps the stored secret, `Some("")` clears it.
    pub secret_access_key: Option<String>,
    pub force_path_style: Option<bool>,
    pub is_default: Option<bool>,
}

/// One object (or common prefix) in a listing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectEntryDto {
    /// Full key for an object; the prefix itself for a folder.
    pub key: String,
    /// Name relative to the listing prefix, for display.
    pub name: String,
    /// `true` for a common prefix ("folder"), which has no size or mtime.
    pub is_prefix: bool,
    pub size_bytes: Option<i64>,
    pub last_modified: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectListingDto {
    pub entries: Vec<ObjectEntryDto>,
    /// Echoed so the console can render a breadcrumb it did not have to guess.
    pub prefix: String,
    /// Continuation token for the next page, absent when the listing is complete.
    pub next_token: Option<String>,
}

/// Outcome of a connectivity probe. A failure is a *result*, not an error:
/// "these credentials do not work" is exactly what the operator asked, so it
/// answers 200 with `ok: false` rather than bubbling a 500 out of the console.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageProbeDto {
    pub ok: bool,
    pub message: String,
}

type ConfigRow = (
    String,
    String, // protocol
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
    String,
    i64,
    i64,
);

const CONFIG_COLS: &str = "id, protocol, config_key, bucket, endpoint, region, prefix, access_key_id, \
                           secret_access_key_encrypted, force_path_style, is_default, created_by, \
                           created_at, updated_at";

fn trimmed(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

/// Normalise a prefix to the one shape the rest of the module assumes: no
/// leading slash, a single trailing slash when non-empty. Without this,
/// `media` and `media/` list different things and `/media` lists nothing.
fn normalize_prefix(raw: &str) -> String {
    let trimmed = raw.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    format!("{}/", trimmed.trim_end_matches('/'))
}

/// Refuse a caller-supplied sub-prefix containing a `..` segment.
///
/// It cannot actually escape — S3 keys are opaque strings, so `media/../`
/// matches nothing rather than climbing out — and MinIO rejects it server-side
/// too. This exists so the operator gets one readable sentence instead of a
/// vendor stack trace, and so the intent is refused at our own boundary rather
/// than relying on every S3 implementation to refuse it for us.
fn reject_traversal(sub: &str) -> Result<(), PlatformError> {
    if sub.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(PlatformError::BadRequest(
            "prefix may not contain \"..\" or \".\" path segments".into(),
        ));
    }
    Ok(())
}

impl PlatformService {
    // -- object storage configs -------------------------------------------

    fn row_to_dto(&self, row: ConfigRow) -> ObjectStorageConfigDto {
        let (
            id,
            protocol,
            config_key,
            bucket,
            endpoint,
            region,
            prefix,
            access_key_id,
            secret_encrypted,
            force_path_style,
            is_default,
            created_by,
            created_at,
            updated_at,
        ) = row;
        ObjectStorageConfigDto {
            id,
            protocol,
            config_key,
            bucket,
            endpoint,
            region,
            prefix,
            access_key_id,
            // The ciphertext stops here.
            has_secret: secret_encrypted.is_some(),
            force_path_style: force_path_style != 0,
            is_default: is_default != 0,
            created_by,
            created_at,
            updated_at,
        }
    }

    pub async fn list_object_storage_configs(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ObjectStorageConfigDto>, PlatformError> {
        let sql = format!(
            "SELECT {CONFIG_COLS} FROM one_object_storage_configs WHERE tenant_id = ? \
             ORDER BY is_default DESC, config_key ASC"
        );
        let rows = self.db.fetch_all_as::<ConfigRow>(&sql, &db_params![tenant_id]).await?;
        Ok(rows.into_iter().map(|r| self.row_to_dto(r)).collect())
    }

    async fn load_config_row(&self, tenant_id: &str, id: &str) -> Result<ConfigRow, PlatformError> {
        let sql = format!("SELECT {CONFIG_COLS} FROM one_object_storage_configs WHERE tenant_id = ? AND id = ?");
        self.db
            .fetch_optional_as::<ConfigRow>(&sql, &db_params![tenant_id, id])
            .await?
            .ok_or_else(|| PlatformError::NotFound(format!("object storage config {id}")))
    }

    /// Exactly one default per tenant. Enforced here rather than by a partial
    /// unique index, which SQLite supports and MySQL does not.
    async fn clear_other_defaults(&self, tenant_id: &str, keep_id: &str) -> Result<(), PlatformError> {
        self.db
            .execute(
                "UPDATE one_object_storage_configs SET is_default = 0 WHERE tenant_id = ? AND id <> ?",
                &db_params![tenant_id, keep_id],
            )
            .await?;
        Ok(())
    }

    pub async fn create_object_storage_config(
        &self,
        tenant_id: &str,
        created_by: &str,
        input: &ObjectStorageConfigInput,
    ) -> Result<ObjectStorageConfigDto, PlatformError> {
        let config_key = trimmed(input.config_key.as_deref())
            .ok_or_else(|| PlatformError::BadRequest("config key is required".into()))?;
        let bucket =
            trimmed(input.bucket.as_deref()).ok_or_else(|| PlatformError::BadRequest("bucket is required".into()))?;
        let endpoint = trimmed(input.endpoint.as_deref())
            .ok_or_else(|| PlatformError::BadRequest("endpoint is required".into()))?;

        // Default to S3: it is what every row written before the driver seam
        // existed speaks, and what the console offers first.
        let protocol = trimmed(input.protocol.as_deref()).unwrap_or_else(|| "s3".into());
        validate_protocol(&protocol)?;

        let existing = self
            .db
            .fetch_optional_as::<(String,)>(
                "SELECT id FROM one_object_storage_configs WHERE tenant_id = ? AND config_key = ?",
                &db_params![tenant_id, &config_key],
            )
            .await?;
        if existing.is_some() {
            return Err(PlatformError::BadRequest(format!(
                "storage config key {config_key} is already used in this tenant"
            )));
        }

        // First config for a tenant becomes the default; otherwise honour the flag.
        let is_first = self.list_object_storage_configs(tenant_id).await?.is_empty();
        let is_default = input.is_default.unwrap_or(false) || is_first;

        let id = generate_prefixed_id("ostor");
        let now = now_ms();
        let secret_cipher = match trimmed(input.secret_access_key.as_deref()) {
            Some(secret) => Some(
                encrypt_string(&secret, &self.encryption_key)
                    .map_err(|e| PlatformError::Internal(format!("failed to encrypt storage secret: {e}")))?,
            ),
            None => None,
        };

        self.db
            .execute(
                "INSERT INTO one_object_storage_configs \
                (id, tenant_id, protocol, config_key, bucket, endpoint, region, prefix, access_key_id, \
                 secret_access_key_encrypted, force_path_style, is_default, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &db_params![
                    &id,
                    tenant_id,
                    &protocol,
                    &config_key,
                    &bucket,
                    &endpoint,
                    trimmed(input.region.as_deref()).unwrap_or_else(|| "us-east-1".into()),
                    input.prefix.as_deref().map(normalize_prefix).filter(|p| !p.is_empty()),
                    trimmed(input.access_key_id.as_deref()),
                    secret_cipher,
                    i64::from(input.force_path_style.unwrap_or(true)),
                    i64::from(is_default),
                    created_by,
                    now,
                    now
                ],
            )
            .await?;

        if is_default {
            self.clear_other_defaults(tenant_id, &id).await?;
        }
        Ok(self.row_to_dto(self.load_config_row(tenant_id, &id).await?))
    }

    pub async fn update_object_storage_config(
        &self,
        tenant_id: &str,
        id: &str,
        input: &ObjectStorageConfigInput,
    ) -> Result<ObjectStorageConfigDto, PlatformError> {
        let current = self.row_to_dto(self.load_config_row(tenant_id, id).await?);

        // Three states in two bound values, same shape as the API-asset editor:
        // flag 0 keeps the stored secret, flag 1 writes `secret` (NULL when the
        // operator cleared it). COALESCE cannot express this.
        let (secret_set, secret_value): (i64, Option<String>) = match input.secret_access_key.as_deref().map(str::trim)
        {
            None => (0, None),
            Some("") => (1, None),
            Some(secret) => (
                1,
                Some(
                    encrypt_string(secret, &self.encryption_key)
                        .map_err(|e| PlatformError::Internal(format!("failed to encrypt storage secret: {e}")))?,
                ),
            ),
        };
        // A protocol change re-points the config at a different driver, so it
        // is validated exactly like a create — and stored, or the row would
        // keep claiming the old one.
        let protocol = match input.protocol.as_deref().map(str::trim) {
            Some(p) if !p.is_empty() => {
                validate_protocol(p)?;
                p.to_owned()
            }
            _ => current.protocol.clone(),
        };
        let is_default = input.is_default.unwrap_or(current.is_default);

        self.db
            .execute(
                "UPDATE one_object_storage_configs SET protocol = ?, bucket = ?, endpoint = ?, region = ?, prefix = ?, \
                 access_key_id = ?, force_path_style = ?, is_default = ?, \
                 secret_access_key_encrypted = CASE WHEN ? = 1 THEN ? ELSE secret_access_key_encrypted END, \
                 updated_at = ? WHERE tenant_id = ? AND id = ?",
                &db_params![
                    &protocol,
                    trimmed(input.bucket.as_deref()).unwrap_or(current.bucket),
                    trimmed(input.endpoint.as_deref()).unwrap_or(current.endpoint),
                    trimmed(input.region.as_deref()).unwrap_or(current.region),
                    match input.prefix.as_deref() {
                        Some(raw) => {
                            let p = normalize_prefix(raw);
                            if p.is_empty() { None } else { Some(p) }
                        }
                        None => current.prefix,
                    },
                    match input.access_key_id.as_deref() {
                        Some(raw) => trimmed(Some(raw)),
                        None => current.access_key_id,
                    },
                    i64::from(input.force_path_style.unwrap_or(current.force_path_style)),
                    i64::from(is_default),
                    secret_set,
                    secret_value,
                    now_ms(),
                    tenant_id,
                    id
                ],
            )
            .await?;

        if is_default {
            self.clear_other_defaults(tenant_id, id).await?;
        }
        Ok(self.row_to_dto(self.load_config_row(tenant_id, id).await?))
    }

    pub async fn delete_object_storage_config(&self, tenant_id: &str, id: &str) -> Result<(), PlatformError> {
        let removed = self.row_to_dto(self.load_config_row(tenant_id, id).await?);
        self.db
            .execute(
                "DELETE FROM one_object_storage_configs WHERE tenant_id = ? AND id = ?",
                &db_params![tenant_id, id],
            )
            .await?;

        // Deleting the default must not leave the tenant without one, or the
        // next "use the default bucket" caller silently gets nothing.
        if removed.is_default
            && let Some(next) = self.list_object_storage_configs(tenant_id).await?.first()
        {
            self.db
                .execute(
                    "UPDATE one_object_storage_configs SET is_default = 1, updated_at = ? \
                         WHERE tenant_id = ? AND id = ?",
                    &db_params![now_ms(), tenant_id, &next.id],
                )
                .await?;
        }
        Ok(())
    }

    pub async fn set_default_object_storage_config(
        &self,
        tenant_id: &str,
        id: &str,
    ) -> Result<ObjectStorageConfigDto, PlatformError> {
        self.load_config_row(tenant_id, id).await?;
        self.db
            .execute(
                "UPDATE one_object_storage_configs SET is_default = 1, updated_at = ? WHERE tenant_id = ? AND id = ?",
                &db_params![now_ms(), tenant_id, id],
            )
            .await?;
        self.clear_other_defaults(tenant_id, id).await?;
        Ok(self.row_to_dto(self.load_config_row(tenant_id, id).await?))
    }

    // -- talking to the storage -------------------------------------------

    /// Resolve one stored config into a driver plus the material it needs,
    /// decrypting the secret at the last possible moment.
    async fn driver_for_config(
        &self,
        tenant_id: &str,
        id: &str,
    ) -> Result<(Box<dyn crate::storage_driver::StorageDriver>, DriverConfig, ConfigRow), PlatformError> {
        let row = self.load_config_row(tenant_id, id).await?;
        let (
            _,
            ref protocol,
            _,
            ref bucket,
            ref endpoint,
            ref region,
            _,
            ref access_key_id,
            ref secret_cipher,
            force_path_style,
            ..,
        ) = row;

        let secret = match secret_cipher {
            Some(cipher) => Some(
                decrypt_string(cipher, &self.encryption_key)
                    .map_err(|e| PlatformError::Internal(format!("failed to decrypt storage secret: {e}")))?,
            ),
            None => None,
        };
        let (Some(key_id), Some(secret)) = (access_key_id.clone(), secret) else {
            return Err(PlatformError::BadRequest(
                "this storage config has no access key and secret configured".into(),
            ));
        };

        let cfg = DriverConfig {
            endpoint: endpoint.clone(),
            bucket: bucket.clone(),
            region: region.clone(),
            access_key_id: key_id,
            secret_access_key: secret,
            force_path_style: force_path_style != 0,
        };
        Ok((driver_for(protocol)?, cfg, row.clone()))
    }

    /// Probe a stored config. A refusal from the storage is reported as
    /// `ok: false` with its own words — the operator asked whether it works,
    /// so "it does not, and here is why" is a successful answer.
    pub async fn probe_object_storage(&self, tenant_id: &str, id: &str) -> Result<StorageProbeDto, PlatformError> {
        let (driver, cfg, _) = match self.driver_for_config(tenant_id, id).await {
            Ok(v) => v,
            Err(PlatformError::BadRequest(msg)) => {
                return Ok(StorageProbeDto {
                    ok: false,
                    message: msg,
                });
            }
            Err(e) => return Err(e),
        };
        match driver.probe(&cfg).await {
            Ok(message) => Ok(StorageProbeDto { ok: true, message }),
            Err(PlatformError::BadRequest(message)) => Ok(StorageProbeDto { ok: false, message }),
            Err(e) => Err(e),
        }
    }

    /// List one level of the storage.
    ///
    /// `sub_prefix` is appended to the config's own prefix, never used in
    /// place of it: a config scoped to `media/` must not be able to browse the
    /// root through this endpoint. Traversal segments are refused here rather
    /// than in each driver, so a new driver cannot forget the check.
    pub async fn list_object_storage_entries(
        &self,
        tenant_id: &str,
        id: &str,
        sub_prefix: Option<&str>,
        token: Option<&str>,
        limit: i32,
    ) -> Result<ObjectListingDto, PlatformError> {
        let (driver, cfg, row) = self.driver_for_config(tenant_id, id).await?;
        let base = row.6.clone().unwrap_or_default();
        let sub = match sub_prefix {
            Some(raw) => {
                reject_traversal(raw)?;
                normalize_prefix(raw)
            }
            None => String::new(),
        };
        let prefix = format!("{base}{sub}");

        let listing = driver.list(&cfg, &prefix, token, limit).await?;
        Ok(ObjectListingDto {
            entries: listing
                .entries
                .into_iter()
                .map(|e| ObjectEntryDto {
                    key: e.key,
                    name: e.name,
                    is_prefix: e.is_prefix,
                    size_bytes: e.size_bytes,
                    last_modified: e.last_modified,
                })
                .collect(),
            prefix,
            next_token: listing.next_token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_segments_are_refused_at_our_boundary() {
        // Cannot actually escape (S3 keys are opaque), but the operator should
        // get a sentence, not a vendor stack trace — and the intent is refused
        // here rather than left to each S3 implementation.
        assert!(reject_traversal("../").is_err());
        assert!(reject_traversal("media/../../etc").is_err());
        assert!(reject_traversal("./x").is_err());
        assert!(reject_traversal("media/sub").is_ok());
        assert!(reject_traversal("").is_ok());
        // A dot inside a name is fine; only a whole segment is a traversal.
        assert!(reject_traversal("2026.09/reports").is_ok());
    }

    #[test]
    fn prefixes_normalise_to_one_shape() {
        // Without this, `media`, `media/` and `/media` list three different
        // things (the last one lists nothing at all).
        assert_eq!(normalize_prefix("media"), "media/");
        assert_eq!(normalize_prefix("media/"), "media/");
        assert_eq!(normalize_prefix("/media"), "media/");
        assert_eq!(normalize_prefix("  media//  "), "media/");
        assert_eq!(normalize_prefix(""), "");
        assert_eq!(normalize_prefix("   "), "");
        assert_eq!(normalize_prefix("/"), "");
    }
}
