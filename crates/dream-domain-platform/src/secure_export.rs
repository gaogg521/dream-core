//! Export watermarking (A) and encrypted outbound packages (B).
//!
//! Design: dream-en `docs/design-2026-10-11-secure-export-and-watermark.zh-CN.md`.
//!
//! Exports happen inside the member's desktop client and never pass through a
//! backend that could gate them, so the client is the enforcement point. This
//! module is the server half:
//!
//! - **Watermark seeds**: one HMAC key per (member, machine). The client signs
//!   each exported Markdown file with it, offline. Only the random seed id
//!   travels in the file; an admin resolves it to a person here.
//! - **Export events**: what left the client, as the client reported it —
//!   names, sizes and a hash, never content.
//! - **Package key escrow**: the client encrypts a package locally and hands
//!   over only its data key. The ciphertext never reaches the server, and a
//!   recipient gets the key back only after the authorization checks in
//!   [`PlatformService::open_secure_package`] pass.
//!
//! The watermark format is shared with the client
//! (`dream-ui … secureExport/watermark.ts`); both sides pin the same test
//! vector, so a change on one side without the other fails a test.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD};
use dream_core_common::{decrypt_field, encrypt_field, generate_prefixed_id, now_ms};
use dream_core_db::db_params;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::PlatformError;
use crate::service::{PlatformActor, PlatformService};

/// Every watermark line starts with this.
pub const WATERMARK_PREFIX: &str = "<!-- ow:wm1.";
const WATERMARK_SUFFIX: &str = "-->";

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// A seed is good for 30 days.
const SEED_TTL_MS: i64 = 30 * DAY_MS;
/// A seed this close to expiry is replaced instead of handed out again, so a
/// client that renews on its sync loop never holds one that dies mid-export.
const SEED_RENEW_WINDOW_MS: i64 = 3 * DAY_MS;
/// Open attempts allowed per member per minute. Generous for a person, but it
/// stops the endpoint from being used to probe package ids.
const OPEN_RATE_LIMIT_PER_MINUTE: i64 = 30;
const MAX_EVENTS_PER_BATCH: usize = 200;
const MAX_FILE_NAMES: usize = 500;
const MAX_LIST_LIMIT: i64 = 500;

/// Where an export came from on the client. Free text would make the audit
/// page unfilterable, so the set is closed.
pub const EXPORT_KINDS: &[&str] = &[
    "conversation_markdown",
    "conversation_command",
    "history_batch",
    "team",
    "preview_download",
    "tool_download",
    "media",
    "backup",
    "clipboard",
];

pub const RECIPIENT_SCOPE_ENTERPRISE: &str = "enterprise";
pub const RECIPIENT_SCOPE_TENANT: &str = "tenant";
pub const RECIPIENT_SCOPE_USERS: &str = "users";

// --- Watermark format -------------------------------------------------------

fn is_hash_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// Whether `line` is a watermark line (surrounding blanks ignored).
pub fn is_watermark_line(line: &str) -> bool {
    let t = line.trim_matches(is_hash_ws);
    t.starts_with(WATERMARK_PREFIX) && t.ends_with(WATERMARK_SUFFIX)
}

/// The text a watermark's hash covers: line endings unified, every watermark
/// line removed, trailing blanks dropped. Only ASCII blanks are trimmed — the
/// JavaScript and Rust "whitespace" sets differ, and the two sides must agree
/// byte for byte.
pub fn normalize_for_hash(text: &str) -> String {
    let unified = text.replace("\r\n", "\n");
    let kept: Vec<&str> = unified.split('\n').filter(|l| !is_watermark_line(l)).collect();
    kept.join("\n").trim_end_matches(is_hash_ws).to_owned()
}

/// First 8 bytes of SHA-256 over [`normalize_for_hash`], lowercase hex.
pub fn content_hash_prefix(text: &str) -> String {
    let digest = Sha256::digest(normalize_for_hash(text).as_bytes());
    hex::encode(&digest[..8])
}

/// First 8 bytes of `HMAC-SHA256(key, "wm1|<seedId>|<ts>|<hash>")`, lowercase hex.
pub fn watermark_mac(key: &[u8], seed_id: &str, ts_base36: &str, hash: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(format!("wm1|{seed_id}|{ts_base36}|{hash}").as_bytes());
    hex::encode(&mac.finalize().into_bytes()[..8])
}

fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_owned();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 digits are ASCII")
}

/// The watermark line for `text` (without a trailing newline). `ts_secs` is
/// the export time in Unix seconds. Server-side use is tests and tooling; the
/// client signs on its own.
pub fn watermark_line(key: &[u8], seed_id: &str, ts_secs: u64, text: &str) -> String {
    let ts = to_base36(ts_secs);
    let hash = content_hash_prefix(text);
    let mac = watermark_mac(key, seed_id, &ts, &hash);
    format!("{WATERMARK_PREFIX}{seed_id}.{ts}.{hash}.{mac} {WATERMARK_SUFFIX}")
}

/// One watermark found in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedWatermark {
    pub seed_id: String,
    pub ts_base36: String,
    pub ts_secs: Option<u64>,
    pub hash: String,
    pub mac: String,
}

/// Every well-formed watermark line in `text`, in order. Malformed lines that
/// merely start with the prefix are skipped rather than reported: a damaged
/// mark proves nothing either way.
pub fn parse_watermarks(text: &str) -> Vec<ParsedWatermark> {
    text.replace("\r\n", "\n")
        .split('\n')
        .filter(|l| is_watermark_line(l))
        .filter_map(|l| {
            let t = l.trim_matches(is_hash_ws);
            let body = t.strip_prefix(WATERMARK_PREFIX)?.strip_suffix(WATERMARK_SUFFIX)?.trim();
            let mut parts = body.split('.');
            let (seed_id, ts, hash, mac) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            if parts.next().is_some()
                || seed_id.is_empty()
                || seed_id.len() > 64
                || !seed_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || ts.is_empty()
                || !ts.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
                || hash.len() != 16
                || mac.len() != 16
                || !hash.bytes().chain(mac.bytes()).all(|b| b.is_ascii_hexdigit())
            {
                return None;
            }
            Some(ParsedWatermark {
                seed_id: seed_id.to_owned(),
                ts_base36: ts.to_owned(),
                ts_secs: u64::from_str_radix(ts, 36).ok(),
                hash: hash.to_ascii_lowercase(),
                mac: mac.to_ascii_lowercase(),
            })
        })
        .collect()
}

/// Key-encryption key for everything this module stores encrypted. Derived
/// from the deployment's data key under its own label, so seed keys and
/// package keys never share a key with provider credentials.
fn secure_export_kek(encryption_key: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"dream-secure-export-kek:v1:");
    hasher.update(encryption_key);
    hasher.finalize().into()
}

fn random_bytes<const N: usize>() -> Result<[u8; N], PlatformError> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|e| PlatformError::Internal(format!("rng failure: {e}")))?;
    Ok(buf)
}

// --- DTOs ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatermarkSeedDto {
    pub seed_id: String,
    /// Base64 of the 32-byte HMAC key. Only ever returned to its owner.
    pub seed_key: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEventInput {
    /// Client-generated; a retried upload of the same event is a no-op.
    pub event_id: String,
    pub kind: String,
    #[serde(default)]
    pub encrypted: bool,
    #[serde(default)]
    pub package_id: Option<String>,
    #[serde(default)]
    pub seed_id: Option<String>,
    #[serde(default)]
    pub file_names: Vec<String>,
    #[serde(default)]
    pub total_bytes: i64,
    #[serde(default)]
    pub content_sha256: Option<String>,
    pub client_ts: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportExportEventsDto {
    /// Events stored by this call (duplicates of earlier uploads excluded).
    pub accepted: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSecurePackageInput {
    pub package_id: String,
    /// Base64 of the 32-byte AES-256-GCM data key.
    pub dek: String,
    pub recipient_scope: String,
    #[serde(default)]
    pub recipient_user_ids: Vec<String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub file_names: Vec<String>,
    #[serde(default)]
    pub total_bytes: i64,
    #[serde(default)]
    pub content_sha256: Option<String>,
    #[serde(default)]
    pub seed_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurePackageDto {
    pub id: String,
    pub owner_user_id: String,
    pub owner_name: String,
    pub tenant_id: String,
    pub recipient_scope: String,
    pub recipient_user_ids: Vec<String>,
    pub file_names: Vec<String>,
    pub total_bytes: i64,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub revoked_by: Option<String>,
    pub open_count: i64,
    /// `"active" | "revoked" | "expired"`.
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSecurePackageDto {
    pub dek: String,
    pub package: SecurePackageDto,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurePackageOpenDto {
    pub id: String,
    pub user_id: String,
    pub user_name: String,
    pub machine_id: Option<String>,
    /// `"granted" | "denied"`.
    pub result: String,
    pub reason: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEventDto {
    pub id: String,
    pub user_id: String,
    pub user_name: String,
    pub machine_id: Option<String>,
    pub kind: String,
    pub encrypted: bool,
    pub package_id: Option<String>,
    pub seed_id: Option<String>,
    pub file_names: Vec<String>,
    pub total_bytes: i64,
    pub content_sha256: Option<String>,
    pub client_ts: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatermarkMatchDto {
    pub seed_id: String,
    /// Export time the mark claims, Unix ms. Only trustworthy when `macValid`.
    pub exported_at: Option<i64>,
    /// Whether the seed belongs to this tenant. `false` = forged, or another
    /// company's mark; nothing else is reported then.
    pub found: bool,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub machine_id: Option<String>,
    /// The signature checks out: this line was produced with that seed.
    pub mac_valid: bool,
    /// The text around the mark is what was exported. `false` with a valid
    /// signature = edited after export, or the line was copied from another
    /// file.
    pub content_matches: bool,
    pub export_event: Option<ExportEventDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatermarkVerifyDto {
    pub content_hash: String,
    pub marks: Vec<WatermarkMatchDto>,
}

/// Filters for the admin package listing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurePackageQuery {
    #[serde(default)]
    pub owner_user_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEventQuery {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

// --- Rows -------------------------------------------------------------------

/// id, enterprise_id, tenant_id, owner_user_id, dek_enc, recipient_scope,
/// recipient_user_ids, file_names, total_bytes, created_at, expires_at,
/// revoked_at, revoked_by.
type PackageRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

const PACKAGE_COLUMNS: &str = "id, enterprise_id, tenant_id, owner_user_id, dek_enc, recipient_scope, \
     recipient_user_ids, file_names, total_bytes, created_at, expires_at, revoked_at, revoked_by";

/// id, user_id, machine_id, kind, encrypted, package_id, seed_id, file_names,
/// total_bytes, content_sha256, client_ts, created_at.
type EventRow = (
    String,
    String,
    Option<String>,
    String,
    bool,
    Option<String>,
    Option<String>,
    String,
    i64,
    Option<String>,
    i64,
    i64,
);

/// id, user_id, machine_id, result, reason, created_at.
type OpenRow = (String, String, Option<String>, String, Option<String>, i64);

const EVENT_COLUMNS: &str = "id, user_id, machine_id, kind, encrypted, package_id, seed_id, file_names, \
     total_bytes, content_sha256, client_ts, created_at";

fn package_status(revoked_at: Option<i64>, expires_at: Option<i64>, now: i64) -> &'static str {
    if revoked_at.is_some() {
        "revoked"
    } else if expires_at.is_some_and(|e| e <= now) {
        "expired"
    } else {
        "active"
    }
}

fn json_list(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn valid_token(id: &str, max: usize) -> bool {
    !id.is_empty() && id.len() <= max && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn clean_file_names(names: &[String]) -> Result<String, PlatformError> {
    if names.len() > MAX_FILE_NAMES {
        return Err(PlatformError::BadRequest(format!(
            "at most {MAX_FILE_NAMES} file names per record"
        )));
    }
    let trimmed: Vec<String> = names.iter().map(|n| n.chars().take(512).collect()).collect();
    serde_json::to_string(&trimmed).map_err(|e| PlatformError::Internal(e.to_string()))
}

impl PlatformService {
    // --- shared helpers ---

    fn secure_export_key(&self) -> [u8; 32] {
        secure_export_kek(&self.encryption_key)
    }

    /// `one_enterprise_members` has one row per user. `None` = not in a company.
    async fn secure_export_enterprise_of(&self, user_id: &str) -> Result<Option<String>, PlatformError> {
        Ok(self
            .db
            .fetch_optional_scalar::<String>(
                "SELECT enterprise_id FROM one_enterprise_members WHERE user_id = ?",
                &db_params![user_id],
            )
            .await?)
    }

    /// The caller's company, refusing callers outside one and plans without
    /// the feature. Never "no company → allowed": that convention is right for
    /// gates that protect personal users, and wrong here, where there is
    /// nothing to hand a personal user.
    async fn secure_export_member(&self, user_id: &str) -> Result<String, PlatformError> {
        let Some(enterprise_id) = self.secure_export_enterprise_of(user_id).await? else {
            return Err(PlatformError::NotInEnterprise);
        };
        self.require_secure_export_plan(user_id).await?;
        Ok(enterprise_id)
    }

    async fn require_secure_export_plan(&self, user_id: &str) -> Result<(), PlatformError> {
        #[cfg(test)]
        if let Some(allowed) = *self.secure_export_plan_override.lock().unwrap() {
            return if allowed {
                Ok(())
            } else {
                Err(PlatformError::PlanRequired("plan override".into()))
            };
        }
        let allowed = dream_core_db::licensed_state::user_feature_allowed(
            &self.db,
            user_id,
            dream_core_common::license::Feature::SecureExport,
        )
        .await
        .map_err(|e| PlatformError::Internal(format!("license check: {e}")))?;
        if allowed {
            Ok(())
        } else {
            Err(PlatformError::PlanRequired(
                "export protection is not included in the current plan".into(),
            ))
        }
    }

    /// Display name for audit and listings: the IdP name when there is one,
    /// else the local username, else the id. Best-effort on every step — a
    /// missing name must never fail the request it decorates.
    async fn secure_export_user_name(&self, user_id: &str) -> String {
        if let Ok(Some(Some(name))) = self
            .db
            .fetch_optional_scalar::<Option<String>>(
                "SELECT display_name FROM one_enterprise_members WHERE user_id = ?",
                &db_params![user_id],
            )
            .await
            && !name.trim().is_empty()
        {
            return name;
        }
        if let Ok(Some(name)) = self
            .db
            .fetch_optional_scalar::<String>("SELECT username FROM users WHERE id = ?", &db_params![user_id])
            .await
            && !name.trim().is_empty()
        {
            return name;
        }
        user_id.to_owned()
    }

    /// Append to one-org's `one_audit_logs`. Best-effort, but a failure is
    /// logged at `warn`: an audit trail that silently stays empty is worse
    /// than none (the MFA audit shipped that way for weeks).
    pub(crate) async fn secure_export_audit(
        &self,
        tenant_id: &str,
        user_id: &str,
        action: &str,
        resource: Option<&str>,
        result: &str,
    ) {
        let username = self.secure_export_user_name(user_id).await;
        let write = self
            .db
            .execute(
                "INSERT INTO one_audit_logs (id, tenant_id, user_id, username, action, resource, result, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                &db_params![
                    generate_prefixed_id("audit"),
                    tenant_id,
                    user_id,
                    username,
                    action,
                    resource,
                    result,
                    now_ms()
                ],
            )
            .await;
        if let Err(e) = write {
            tracing::warn!(error = %e, action, "secure-export audit write failed");
        }
    }

    // --- watermark seeds ---

    /// The seed this machine should sign with: the current one while it has
    /// more than [`SEED_RENEW_WINDOW_MS`] left, otherwise a fresh one.
    pub async fn issue_watermark_seed(
        &self,
        actor: &PlatformActor,
        user_id: &str,
        machine_id: &str,
    ) -> Result<WatermarkSeedDto, PlatformError> {
        let enterprise_id = self.secure_export_member(user_id).await?;
        let machine_id = machine_id.trim();
        if !valid_token(machine_id, 255) && machine_id != "unknown" {
            return Err(PlatformError::BadRequest("invalid machine id".into()));
        }
        let now = now_ms();
        let key = self.secure_export_key();
        let existing: Option<(String, String, i64)> = self
            .db
            .fetch_optional_as(
                "SELECT id, key_enc, expires_at FROM one_watermark_seeds \
                 WHERE user_id = ? AND machine_id = ? AND enterprise_id = ? AND expires_at > ? \
                 ORDER BY expires_at DESC LIMIT 1",
                &db_params![user_id, machine_id, &enterprise_id, now + SEED_RENEW_WINDOW_MS],
            )
            .await?;
        if let Some((seed_id, key_enc, expires_at)) = existing {
            let seed_key = decrypt_field(&key_enc, &key)
                .map_err(|e| PlatformError::Internal(format!("watermark seed unreadable: {e}")))?;
            return Ok(WatermarkSeedDto {
                seed_id,
                seed_key,
                expires_at,
            });
        }
        let seed_id = URL_SAFE_NO_PAD.encode(random_bytes::<12>()?);
        let seed_key = BASE64.encode(random_bytes::<32>()?);
        let key_enc = encrypt_field(&seed_key, &key).map_err(|e| PlatformError::Internal(e.to_string()))?;
        let expires_at = now + SEED_TTL_MS;
        self.db
            .execute(
                "INSERT INTO one_watermark_seeds \
                 (id, enterprise_id, tenant_id, user_id, machine_id, key_enc, created_at, expires_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                &db_params![
                    &seed_id,
                    &enterprise_id,
                    &actor.tenant_id,
                    user_id,
                    machine_id,
                    &key_enc,
                    now,
                    expires_at
                ],
            )
            .await?;
        tracing::info!(tenant = %actor.tenant_id, seed = %seed_id, "watermark seed issued");
        Ok(WatermarkSeedDto {
            seed_id,
            seed_key,
            expires_at,
        })
    }

    // --- export events ---

    pub async fn report_export_events(
        &self,
        actor: &PlatformActor,
        user_id: &str,
        machine_id: Option<&str>,
        events: Vec<ExportEventInput>,
    ) -> Result<ReportExportEventsDto, PlatformError> {
        let enterprise_id = self.secure_export_member(user_id).await?;
        if events.len() > MAX_EVENTS_PER_BATCH {
            return Err(PlatformError::BadRequest(format!(
                "at most {MAX_EVENTS_PER_BATCH} events per upload"
            )));
        }
        // Validate the whole batch first, so a bad entry does not leave half
        // of it stored and the rest retried forever.
        for e in &events {
            if !valid_token(&e.event_id, 64) {
                return Err(PlatformError::BadRequest("invalid eventId".into()));
            }
            if !EXPORT_KINDS.contains(&e.kind.as_str()) {
                return Err(PlatformError::BadRequest(format!("unknown export kind '{}'", e.kind)));
            }
            if e.package_id.as_deref().is_some_and(|p| !valid_token(p, 64))
                || e.seed_id.as_deref().is_some_and(|s| !valid_token(s, 64))
            {
                return Err(PlatformError::BadRequest("invalid packageId or seedId".into()));
            }
            if e.content_sha256
                .as_deref()
                .is_some_and(|h| h.len() > 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(PlatformError::BadRequest("contentSha256 must be hex".into()));
            }
            if e.total_bytes < 0 {
                return Err(PlatformError::BadRequest("totalBytes must not be negative".into()));
            }
        }
        let now = now_ms();
        let mut accepted = 0;
        for e in events {
            let file_names = clean_file_names(&e.file_names)?;
            let inserted = self
                .upsert(
                    "INSERT OR IGNORE INTO one_export_events \
                     (id, enterprise_id, tenant_id, user_id, machine_id, kind, encrypted, package_id, seed_id, \
                      file_names, total_bytes, content_sha256, client_ts, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    "INSERT IGNORE INTO one_export_events \
                     (id, enterprise_id, tenant_id, user_id, machine_id, kind, encrypted, package_id, seed_id, \
                      file_names, total_bytes, content_sha256, client_ts, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    &db_params![
                        &e.event_id,
                        &enterprise_id,
                        &actor.tenant_id,
                        user_id,
                        machine_id,
                        &e.kind,
                        e.encrypted,
                        e.package_id.as_deref(),
                        e.seed_id.as_deref(),
                        &file_names,
                        e.total_bytes,
                        e.content_sha256.as_deref(),
                        e.client_ts,
                        now
                    ],
                )
                .await?;
            if inserted == 0 {
                continue;
            }
            accepted += 1;
            let action = if e.kind == "clipboard" {
                "secure_export.clipboard.copy"
            } else {
                "secure_export.export"
            };
            let resource = format!(
                "{}:{}{}",
                e.kind,
                if e.encrypted { "encrypted:" } else { "" },
                json_list(&file_names).join(",")
            );
            self.secure_export_audit(&actor.tenant_id, user_id, action, Some(&resource), "success")
                .await;
        }
        Ok(ReportExportEventsDto { accepted })
    }

    pub async fn list_export_events(
        &self,
        tenant_id: &str,
        query: &ExportEventQuery,
    ) -> Result<Vec<ExportEventDto>, PlatformError> {
        let limit = query.limit.unwrap_or(200).clamp(1, MAX_LIST_LIMIT);
        let rows: Vec<EventRow> = self
            .db
            .fetch_all_as(
                &format!(
                    "SELECT {EVENT_COLUMNS} FROM one_export_events \
                     WHERE tenant_id = ? AND (? IS NULL OR user_id = ?) AND (? IS NULL OR kind = ?) \
                     ORDER BY created_at DESC LIMIT ?"
                ),
                &db_params![
                    tenant_id,
                    query.user_id.as_deref(),
                    query.user_id.as_deref(),
                    query.kind.as_deref(),
                    query.kind.as_deref(),
                    limit
                ],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(self.event_row_to_dto(row).await);
        }
        Ok(out)
    }

    async fn event_row_to_dto(&self, row: EventRow) -> ExportEventDto {
        ExportEventDto {
            user_name: self.secure_export_user_name(&row.1).await,
            id: row.0,
            user_id: row.1,
            machine_id: row.2,
            kind: row.3,
            encrypted: row.4,
            package_id: row.5,
            seed_id: row.6,
            file_names: json_list(&row.7),
            total_bytes: row.8,
            content_sha256: row.9,
            client_ts: row.10,
            created_at: row.11,
        }
    }

    // --- encrypted packages ---

    pub async fn create_secure_package(
        &self,
        actor: &PlatformActor,
        user_id: &str,
        machine_id: Option<&str>,
        input: CreateSecurePackageInput,
    ) -> Result<SecurePackageDto, PlatformError> {
        let enterprise_id = self.secure_export_member(user_id).await?;
        if !valid_token(&input.package_id, 64) {
            return Err(PlatformError::BadRequest("invalid packageId".into()));
        }
        let dek = BASE64
            .decode(input.dek.trim())
            .map_err(|_| PlatformError::BadRequest("dek must be base64".into()))?;
        if dek.len() != 32 {
            return Err(PlatformError::BadRequest("dek must be 32 bytes".into()));
        }
        let scope = input.recipient_scope.as_str();
        if !matches!(
            scope,
            RECIPIENT_SCOPE_ENTERPRISE | RECIPIENT_SCOPE_TENANT | RECIPIENT_SCOPE_USERS
        ) {
            return Err(PlatformError::BadRequest(format!(
                "recipientScope must be enterprise, tenant or users, got '{scope}'"
            )));
        }
        let mut recipients: Vec<String> = Vec::new();
        if scope == RECIPIENT_SCOPE_USERS {
            for id in &input.recipient_user_ids {
                let id = id.trim();
                if !id.is_empty() && !recipients.iter().any(|r| r == id) {
                    recipients.push(id.to_owned());
                }
            }
            if recipients.is_empty() {
                return Err(PlatformError::BadRequest(
                    "recipientScope 'users' needs at least one recipient".into(),
                ));
            }
            // Everyone named must be in this company: a key handed to an id
            // that can never authenticate here is a package nobody can open.
            for id in &recipients {
                if self.secure_export_enterprise_of(id).await?.as_deref() != Some(enterprise_id.as_str()) {
                    return Err(PlatformError::BadRequest(format!(
                        "recipient '{id}' is not a member of this company"
                    )));
                }
            }
        }
        let now = now_ms();
        if input.expires_at.is_some_and(|e| e <= now) {
            return Err(PlatformError::BadRequest("expiresAt must be in the future".into()));
        }
        if input.total_bytes < 0 {
            return Err(PlatformError::BadRequest("totalBytes must not be negative".into()));
        }
        if input.seed_id.as_deref().is_some_and(|s| !valid_token(s, 64)) {
            return Err(PlatformError::BadRequest("invalid seedId".into()));
        }
        if input
            .content_sha256
            .as_deref()
            .is_some_and(|h| h.len() > 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(PlatformError::BadRequest("contentSha256 must be hex".into()));
        }
        let file_names = clean_file_names(&input.file_names)?;
        let recipients_json = serde_json::to_string(&recipients).map_err(|e| PlatformError::Internal(e.to_string()))?;
        let dek_enc = encrypt_field(&BASE64.encode(&dek), &self.secure_export_key())
            .map_err(|e| PlatformError::Internal(e.to_string()))?;
        let inserted = self
            .upsert(
                "INSERT OR IGNORE INTO one_secure_export_packages \
                 (id, enterprise_id, tenant_id, owner_user_id, machine_id, dek_enc, recipient_scope, \
                  recipient_user_ids, file_names, total_bytes, content_sha256, seed_id, created_at, expires_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                "INSERT IGNORE INTO one_secure_export_packages \
                 (id, enterprise_id, tenant_id, owner_user_id, machine_id, dek_enc, recipient_scope, \
                  recipient_user_ids, file_names, total_bytes, content_sha256, seed_id, created_at, expires_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &db_params![
                    &input.package_id,
                    &enterprise_id,
                    &actor.tenant_id,
                    user_id,
                    machine_id,
                    &dek_enc,
                    scope,
                    &recipients_json,
                    &file_names,
                    input.total_bytes,
                    input.content_sha256.as_deref(),
                    input.seed_id.as_deref(),
                    now,
                    input.expires_at
                ],
            )
            .await?;
        if inserted == 0 {
            // Package ids are client-minted UUIDs; a repeat is either a bug
            // or someone trying to swap the key behind an existing package.
            return Err(PlatformError::BadRequest(
                "a package with this id already exists".into(),
            ));
        }
        self.secure_export_audit(
            &actor.tenant_id,
            user_id,
            "secure_export.package.create",
            Some(&format!("{}:{}", input.package_id, json_list(&file_names).join(","))),
            "success",
        )
        .await;
        self.package_dto(&input.package_id).await
    }

    async fn package_row(&self, package_id: &str) -> Result<Option<PackageRow>, PlatformError> {
        Ok(self
            .db
            .fetch_optional_as::<PackageRow>(
                &format!("SELECT {PACKAGE_COLUMNS} FROM one_secure_export_packages WHERE id = ?"),
                &db_params![package_id],
            )
            .await?)
    }

    async fn package_row_to_dto(&self, row: PackageRow) -> Result<SecurePackageDto, PlatformError> {
        let open_count: i64 = self
            .db
            .fetch_one_scalar(
                "SELECT COUNT(*) FROM one_secure_export_opens WHERE package_id = ? AND result = 'granted'",
                &db_params![&row.0],
            )
            .await?;
        Ok(SecurePackageDto {
            owner_name: self.secure_export_user_name(&row.3).await,
            status: package_status(row.11, row.10, now_ms()).to_owned(),
            id: row.0,
            owner_user_id: row.3,
            tenant_id: row.2,
            recipient_scope: row.5,
            recipient_user_ids: json_list(&row.6),
            file_names: json_list(&row.7),
            total_bytes: row.8,
            created_at: row.9,
            expires_at: row.10,
            revoked_at: row.11,
            revoked_by: row.12,
            open_count,
        })
    }

    async fn package_dto(&self, package_id: &str) -> Result<SecurePackageDto, PlatformError> {
        let row = self
            .package_row(package_id)
            .await?
            .ok_or_else(|| PlatformError::NotFound("package not found".into()))?;
        self.package_row_to_dto(row).await
    }

    async fn record_open(
        &self,
        package_id: &str,
        user_id: &str,
        machine_id: Option<&str>,
        result: &str,
        reason: Option<&str>,
    ) -> Result<(), PlatformError> {
        self.db
            .execute(
                "INSERT INTO one_secure_export_opens (id, package_id, user_id, machine_id, result, reason, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                &db_params![
                    generate_prefixed_id("sxo"),
                    package_id,
                    user_id,
                    machine_id,
                    result,
                    reason,
                    now_ms()
                ],
            )
            .await?;
        Ok(())
    }

    /// Hand the data key back to an authorized recipient. Checks, in order:
    /// company and plan, rate limit, package exists in the caller's company
    /// (404 otherwise — existence is not disclosed across companies), not
    /// revoked, not expired, caller within the recipient scope. The owner is
    /// always within scope. Every decision after the existence check is
    /// recorded, granted or not.
    pub async fn open_secure_package(
        &self,
        actor: &PlatformActor,
        user_id: &str,
        machine_id: Option<&str>,
        package_id: &str,
    ) -> Result<OpenSecurePackageDto, PlatformError> {
        let enterprise_id = self.secure_export_member(user_id).await?;
        if !valid_token(package_id, 64) {
            return Err(PlatformError::NotFound("package not found".into()));
        }
        let recent: i64 = self
            .db
            .fetch_one_scalar(
                "SELECT COUNT(*) FROM one_secure_export_opens WHERE user_id = ? AND created_at > ?",
                &db_params![user_id, now_ms() - 60_000],
            )
            .await?;
        if recent >= OPEN_RATE_LIMIT_PER_MINUTE {
            return Err(PlatformError::RateLimited(
                "too many open attempts, try again in a minute".into(),
            ));
        }
        let row = match self.package_row(package_id).await? {
            Some(row) if row.1 == enterprise_id => row,
            _ => return Err(PlatformError::NotFound("package not found".into())),
        };
        let refusal: Option<&'static str> = if row.11.is_some() {
            Some("revoked")
        } else if row.10.is_some_and(|e| e <= now_ms()) {
            Some("expired")
        } else if row.3 == user_id {
            None
        } else {
            match row.5.as_str() {
                RECIPIENT_SCOPE_ENTERPRISE => None,
                RECIPIENT_SCOPE_TENANT => {
                    let member: Option<String> = self
                        .db
                        .fetch_optional_scalar(
                            "SELECT tenant_id FROM one_user_org WHERE user_id = ? AND tenant_id = ?",
                            &db_params![user_id, &row.2],
                        )
                        .await?;
                    if member.is_some() { None } else { Some("not_recipient") }
                }
                _ => {
                    if json_list(&row.6).iter().any(|r| r == user_id) {
                        None
                    } else {
                        Some("not_recipient")
                    }
                }
            }
        };
        if let Some(reason) = refusal {
            self.record_open(package_id, user_id, machine_id, "denied", Some(reason))
                .await?;
            self.secure_export_audit(
                &actor.tenant_id,
                user_id,
                "secure_export.package.open_denied",
                Some(&format!("{package_id}:{reason}")),
                "failure",
            )
            .await;
            return Err(match reason {
                "revoked" => {
                    PlatformError::PackageUnavailable("PACKAGE_REVOKED", "the sender revoked this file".into())
                }
                "expired" => PlatformError::PackageUnavailable("PACKAGE_EXPIRED", "this file has expired".into()),
                _ => PlatformError::Forbidden("you are not a recipient of this file".into()),
            });
        }
        let dek = decrypt_field(&row.4, &self.secure_export_key())
            .map_err(|e| PlatformError::Internal(format!("package key unreadable: {e}")))?;
        self.record_open(package_id, user_id, machine_id, "granted", None)
            .await?;
        self.secure_export_audit(
            &actor.tenant_id,
            user_id,
            "secure_export.package.open",
            Some(package_id),
            "success",
        )
        .await;
        Ok(OpenSecurePackageDto {
            dek,
            package: self.package_row_to_dto(row).await?,
        })
    }

    pub async fn list_my_secure_packages(&self, user_id: &str) -> Result<Vec<SecurePackageDto>, PlatformError> {
        self.secure_export_member(user_id).await?;
        let rows: Vec<PackageRow> = self
            .db
            .fetch_all_as(
                &format!(
                    "SELECT {PACKAGE_COLUMNS} FROM one_secure_export_packages \
                     WHERE owner_user_id = ? ORDER BY created_at DESC LIMIT ?"
                ),
                &db_params![user_id, MAX_LIST_LIMIT],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(self.package_row_to_dto(row).await?);
        }
        Ok(out)
    }

    pub async fn list_secure_packages(
        &self,
        tenant_id: &str,
        query: &SecurePackageQuery,
    ) -> Result<Vec<SecurePackageDto>, PlatformError> {
        let limit = query.limit.unwrap_or(200).clamp(1, MAX_LIST_LIMIT);
        let rows: Vec<PackageRow> = self
            .db
            .fetch_all_as(
                &format!(
                    "SELECT {PACKAGE_COLUMNS} FROM one_secure_export_packages \
                     WHERE tenant_id = ? AND (? IS NULL OR owner_user_id = ?) \
                     ORDER BY created_at DESC LIMIT ?"
                ),
                &db_params![
                    tenant_id,
                    query.owner_user_id.as_deref(),
                    query.owner_user_id.as_deref(),
                    limit
                ],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let dto = self.package_row_to_dto(row).await?;
            if query.status.as_deref().is_none_or(|s| s == dto.status) {
                out.push(dto);
            }
        }
        Ok(out)
    }

    /// Admin-side lookup scoped to the admin's tenant; 404 outside it.
    async fn tenant_package_row(&self, tenant_id: &str, package_id: &str) -> Result<PackageRow, PlatformError> {
        match self.package_row(package_id).await? {
            Some(row) if row.2 == tenant_id => Ok(row),
            _ => Err(PlatformError::NotFound("package not found".into())),
        }
    }

    /// Owner-side lookup; 404 for anyone else's package.
    async fn owned_package_row(&self, user_id: &str, package_id: &str) -> Result<PackageRow, PlatformError> {
        match self.package_row(package_id).await? {
            Some(row) if row.3 == user_id => Ok(row),
            _ => Err(PlatformError::NotFound("package not found".into())),
        }
    }

    async fn package_opens(&self, package_id: &str) -> Result<Vec<SecurePackageOpenDto>, PlatformError> {
        let rows: Vec<OpenRow> = self
            .db
            .fetch_all_as(
                "SELECT id, user_id, machine_id, result, reason, created_at FROM one_secure_export_opens \
                 WHERE package_id = ? ORDER BY created_at DESC LIMIT ?",
                &db_params![package_id, MAX_LIST_LIMIT],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, user_id, machine_id, result, reason, created_at) in rows {
            out.push(SecurePackageOpenDto {
                user_name: self.secure_export_user_name(&user_id).await,
                id,
                user_id,
                machine_id,
                result,
                reason,
                created_at,
            });
        }
        Ok(out)
    }

    pub async fn my_secure_package_opens(
        &self,
        user_id: &str,
        package_id: &str,
    ) -> Result<Vec<SecurePackageOpenDto>, PlatformError> {
        self.secure_export_member(user_id).await?;
        self.owned_package_row(user_id, package_id).await?;
        self.package_opens(package_id).await
    }

    pub async fn admin_secure_package_opens(
        &self,
        tenant_id: &str,
        package_id: &str,
    ) -> Result<Vec<SecurePackageOpenDto>, PlatformError> {
        self.tenant_package_row(tenant_id, package_id).await?;
        self.package_opens(package_id).await
    }

    async fn revoke_package_row(
        &self,
        tenant_id: &str,
        actor_user_id: &str,
        row: PackageRow,
    ) -> Result<SecurePackageDto, PlatformError> {
        if row.11.is_none() {
            self.db
                .execute(
                    "UPDATE one_secure_export_packages SET revoked_at = ?, revoked_by = ? \
                     WHERE id = ? AND revoked_at IS NULL",
                    &db_params![now_ms(), actor_user_id, &row.0],
                )
                .await?;
            self.secure_export_audit(
                tenant_id,
                actor_user_id,
                "secure_export.package.revoke",
                Some(&row.0),
                "success",
            )
            .await;
        }
        self.package_dto(&row.0).await
    }

    pub async fn revoke_my_secure_package(
        &self,
        actor: &PlatformActor,
        user_id: &str,
        package_id: &str,
    ) -> Result<SecurePackageDto, PlatformError> {
        self.secure_export_member(user_id).await?;
        let row = self.owned_package_row(user_id, package_id).await?;
        self.revoke_package_row(&actor.tenant_id, user_id, row).await
    }

    pub async fn admin_revoke_secure_package(
        &self,
        tenant_id: &str,
        admin_user_id: &str,
        package_id: &str,
    ) -> Result<SecurePackageDto, PlatformError> {
        let row = self.tenant_package_row(tenant_id, package_id).await?;
        self.revoke_package_row(tenant_id, admin_user_id, row).await
    }

    // --- watermark verification ---

    /// Resolve every watermark in `text` to the member and machine it was
    /// issued to, and say whether each one is genuine and still sits on the
    /// content it was made for. Seeds outside the admin's tenant are reported
    /// as not found, never resolved.
    pub async fn verify_watermarks(
        &self,
        tenant_id: &str,
        admin_user_id: &str,
        text: &str,
    ) -> Result<WatermarkVerifyDto, PlatformError> {
        let content_hash = content_hash_prefix(text);
        let key = self.secure_export_key();
        let mut marks = Vec::new();
        for mark in parse_watermarks(text) {
            let seed: Option<(String, String, String, String)> = self
                .db
                .fetch_optional_as(
                    "SELECT tenant_id, user_id, machine_id, key_enc FROM one_watermark_seeds WHERE id = ?",
                    &db_params![&mark.seed_id],
                )
                .await?;
            let exported_at = mark.ts_secs.and_then(|s| i64::try_from(s).ok()).map(|s| s * 1000);
            let Some((seed_tenant, user_id, machine_id, key_enc)) = seed.filter(|s| s.0 == tenant_id) else {
                marks.push(WatermarkMatchDto {
                    seed_id: mark.seed_id,
                    exported_at,
                    found: false,
                    user_id: None,
                    user_name: None,
                    machine_id: None,
                    mac_valid: false,
                    content_matches: false,
                    export_event: None,
                });
                continue;
            };
            let _ = seed_tenant;
            let seed_key = decrypt_field(&key_enc, &key)
                .ok()
                .and_then(|k| BASE64.decode(k).ok())
                .unwrap_or_default();
            let mac_valid = !seed_key.is_empty()
                && watermark_mac(&seed_key, &mark.seed_id, &mark.ts_base36, &mark.hash) == mark.mac;
            // The export this mark came from, when the client reported one:
            // same seed, client time within a minute of the mark's time.
            let event = match exported_at {
                Some(ts) => {
                    let row: Option<EventRow> = self
                        .db
                        .fetch_optional_as(
                            &format!(
                                "SELECT {EVENT_COLUMNS} FROM one_export_events \
                                 WHERE seed_id = ? AND client_ts BETWEEN ? AND ? \
                                 ORDER BY client_ts ASC LIMIT 1"
                            ),
                            &db_params![&mark.seed_id, ts - 60_000, ts + 60_000],
                        )
                        .await?;
                    match row {
                        Some(row) => Some(self.event_row_to_dto(row).await),
                        None => None,
                    }
                }
                None => None,
            };
            marks.push(WatermarkMatchDto {
                content_matches: mark.hash == content_hash,
                user_name: Some(self.secure_export_user_name(&user_id).await),
                seed_id: mark.seed_id,
                exported_at,
                found: true,
                user_id: Some(user_id),
                machine_id: Some(machine_id),
                mac_valid,
                export_event: event,
            });
        }
        self.secure_export_audit(
            tenant_id,
            admin_user_id,
            "secure_export.watermark.verify",
            Some(&format!("marks={}", marks.len())),
            "success",
        )
        .await;
        Ok(WatermarkVerifyDto { content_hash, marks })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned test vector shared with the client implementation
    /// (dream-ui `secureExport/watermark.test.ts`). Changing the format means
    /// changing both, deliberately.
    const VECTOR_KEY: [u8; 32] = [7u8; 32];
    const VECTOR_SEED: &str = "AAECAwQFBgcICQoL";
    const VECTOR_TS: u64 = 1_760_000_000;
    const VECTOR_TEXT: &str = "# 标题\r\n\r\nHello world.  \n\n";

    #[test]
    fn shared_test_vector() {
        assert_eq!(normalize_for_hash(VECTOR_TEXT), "# 标题\n\nHello world.");
        let line = watermark_line(&VECTOR_KEY, VECTOR_SEED, VECTOR_TS, VECTOR_TEXT);
        assert_eq!(
            line,
            format!(
                "<!-- ow:wm1.AAECAwQFBgcICQoL.{}.{}.{} -->",
                to_base36(VECTOR_TS),
                content_hash_prefix(VECTOR_TEXT),
                watermark_mac(
                    &VECTOR_KEY,
                    VECTOR_SEED,
                    &to_base36(VECTOR_TS),
                    &content_hash_prefix(VECTOR_TEXT)
                )
            )
        );
        // The literal values, so the client test can pin the same bytes.
        assert_eq!(to_base36(VECTOR_TS), "t3uwow");
        assert_eq!(content_hash_prefix(VECTOR_TEXT), VECTOR_HASH);
        assert_eq!(line, VECTOR_LINE);
    }

    const VECTOR_HASH: &str = "89020028383549a1";
    const VECTOR_LINE: &str = "<!-- ow:wm1.AAECAwQFBgcICQoL.t3uwow.89020028383549a1.d0d5355d4b0c11fb -->";

    #[test]
    fn hash_ignores_existing_marks_and_line_endings() {
        let stamped = format!("{VECTOR_TEXT}\n{}\n", watermark_line(&VECTOR_KEY, "x", 1, VECTOR_TEXT));
        assert_eq!(content_hash_prefix(&stamped), content_hash_prefix(VECTOR_TEXT));
        assert_eq!(
            content_hash_prefix("a\r\nb"),
            content_hash_prefix("a\nb\n\n  "),
            "CRLF and trailing blanks must not change the hash"
        );
        assert_ne!(content_hash_prefix("a\nb"), content_hash_prefix("a\nc"));
    }

    #[test]
    fn parse_round_trips_and_rejects_damage() {
        let line = watermark_line(&VECTOR_KEY, VECTOR_SEED, VECTOR_TS, "body");
        let doc = format!("body\n\n{line}\n");
        let marks = parse_watermarks(&doc);
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].seed_id, VECTOR_SEED);
        assert_eq!(marks[0].ts_secs, Some(VECTOR_TS));
        assert_eq!(
            watermark_mac(&VECTOR_KEY, &marks[0].seed_id, &marks[0].ts_base36, &marks[0].hash),
            marks[0].mac
        );
        assert!(parse_watermarks("<!-- ow:wm1.abc.1.zz.zz -->").is_empty());
        assert!(parse_watermarks("<!-- ow:wm1.a.b.c.d.e -->").is_empty());
        assert!(parse_watermarks("<!-- comment -->").is_empty());
    }

    #[test]
    fn kek_is_domain_separated() {
        let base = [9u8; 32];
        assert_ne!(secure_export_kek(&base), base);
        assert_eq!(secure_export_kek(&base), secure_export_kek(&base));
    }

    #[test]
    fn status_precedence() {
        assert_eq!(package_status(Some(1), Some(0), 10), "revoked");
        assert_eq!(package_status(None, Some(5), 10), "expired");
        assert_eq!(package_status(None, Some(50), 10), "active");
        assert_eq!(package_status(None, None, 10), "active");
    }
}
