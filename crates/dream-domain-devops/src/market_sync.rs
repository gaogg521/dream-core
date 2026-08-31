//! Remote content market (P1-1 round 2, align-openocta §4): admin-curated
//! HTTP(S) sources synced into the skill/MCP registries.
//!
//! The trust model lives in `015_market_sync.sql`'s doc comment — one
//! paragraph, the short version: nothing ships pre-trusted, a source exists
//! because an administrator typed its URL in (the same act and trust level
//! as a hand upload), import-time validation is the SAME frontmatter parser
//! uploads use, and there is deliberately no import-time sandbox — imported
//! content is data, and the runtime policy stack (send gate, terminal-tool
//! security gate, content inspection) is what governs whatever agents later
//! do with it.
//!
//! Wire format: an `index.json` manifest at the source URL —
//!
//! ```json
//! {
//!   "version": 1,
//!   "items": [
//!     { "kind": "skill", "name": "code-review",
//!       "path": "skills/code-review/SKILL.md" },
//!     { "kind": "mcp", "name": "jira",
//!       "mcp": { "type": "sse", "endpoint": "https://jira.example.com/mcp" } }
//!   ]
//! }
//! ```
//!
//! For skills the SKILL.md frontmatter is authoritative (name must match the
//! manifest item name — the mapping key has to stay stable across syncs);
//! manifest `title`/`description` are advisory. MCP items carry their
//! type/endpoint inline (CHECK-constrained to stdio/sse like every other
//! row) and never carry secrets — `has_keys` stays 0, credentials are a
//! per-deployment concern, not something a manifest distributes.
//!
//! Incremental semantics: per-item SHA-256 against `one_market_imports`,
//! optionally pinned IN the manifest (`sha256` per item): a matching pin
//! skips the item without fetching, a pin disagreeing with the fetched
//! payload is an integrity error (tamper detection), no pin → fetch and
//! compare the content hash as before. Same hash → skipped; changed → fetched and upserted
//! (UPDATE by the mapping's registry id, `origin` untouched — it is already
//! 'market'); a name colliding with a row this source did not import is an
//! ITEM error, never a takeover (D7 uniqueness + the no-hijack rule the
//! P1-6 publish path enforces); an item missing from the upstream index is
//! reported and its local row KEPT — published content must not vanish
//! because upstream shuffled its manifest.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use dream_core_db::{DbPool, db_params};

use crate::error::DevopsError;
use crate::service::{DevopsService, new_id};
use dream_core_common::now_ms;

/// Cap on any single fetched payload (manifest or skill file). Skills are
/// small text files; 2 MiB is generous and keeps a hostile source from
/// streaming gigabytes into memory. The request body limit for uploads
/// (`BODY_LIMIT`, 10 MiB) is the analogous ceiling on the manual path.
const FETCH_CAP_BYTES: usize = 2 * 1024 * 1024;

/// The kinds a manifest may carry. Digital-employee templates are a
/// deliberate v1 exclusion: employees are owner-centric assets, and what a
/// market template means for ownership is the P1-2 design, not a sync detail.
const MARKET_KINDS: [&str; 2] = ["skill", "mcp"];

/// Fetches remote bytes. A trait so tests inject a fake and never touch the
/// network; the production impl is [`ReqwestFetcher`].
#[async_trait]
pub trait MarketFetcher: Send + Sync {
    async fn fetch(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String>;
}

/// Production fetcher: one shared client, hard total-request timeout, and
/// the size cap enforced on the header (cheap) AND the body (authoritative —
/// a lying Content-Length must not help).
pub struct ReqwestFetcher {
    client: reqwest::Client,
}

impl ReqwestFetcher {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for ReqwestFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MarketFetcher for ReqwestFetcher {
    async fn fetch(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        if let Some(len) = response.content_length() {
            if len as usize > max_bytes {
                return Err(format!("payload too large ({len} bytes, cap {max_bytes})"));
            }
        }
        let bytes = response.bytes().await.map_err(|e| format!("body read failed: {e}"))?;
        if bytes.len() > max_bytes {
            return Err(format!("payload too large ({} bytes, cap {max_bytes})", bytes.len()));
        }
        Ok(bytes.to_vec())
    }
}

/// One manifest entry. `title`/`description`/`version`/`category` are
/// advisory except for MCP items, whose inline `mcp` object is the payload.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestItem {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    /// Relative to the manifest URL's directory. Absolute URLs and traversal
    /// are item errors, not sync failures.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub mcp: Option<ManifestMcp>,
    /// Optional integrity pin (hex SHA-256 of the item's payload). When the
    /// recorded mapping hash matches, the sync skips the item WITHOUT
    /// fetching; when it differs (or the item is new), the fetched payload's
    /// hash must equal the pin or the item errors — a publisher-pinned
    /// manifest doubles as tamper detection.
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMcp {
    /// CHECK-constrained to `stdio` | `sse` on the table.
    pub r#type: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MarketManifest {
    pub version: i64,
    pub items: Vec<ManifestItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSourceDto {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub last_synced_at: Option<i64>,
    /// `"ok" | "error"`, absent until the first sync.
    pub last_sync_status: Option<String>,
    pub last_sync_error: Option<String>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSyncReportDto {
    pub source_id: String,
    pub total_indexed: usize,
    pub imported: u32,
    pub updated: u32,
    pub skipped: u32,
    /// `kind:name` entries present locally but gone from the upstream index.
    /// Local rows are KEPT — removal is an explicit admin action.
    pub removed: Vec<String>,
    /// Per-item failures; a bad item never fails the whole sync.
    pub errors: Vec<MarketSyncItemError>,
    pub synced_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSyncItemError {
    pub item: String,
    pub error: String,
}

type SourceRow = (
    String,
    String,
    String,
    String,
    bool,
    Option<i64>,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
);

fn row_to_source(row: SourceRow) -> MarketSourceDto {
    let (
        id,
        _tenant,
        name,
        url,
        enabled,
        last_synced_at,
        last_sync_status,
        last_sync_error,
        created_by,
        created_at,
        updated_at,
    ) = row;
    MarketSourceDto {
        id,
        name,
        url,
        enabled,
        last_synced_at,
        last_sync_status,
        last_sync_error,
        created_by,
        created_at,
        updated_at,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Resolve a manifest-relative path against the manifest URL's directory.
/// Absolute URLs and `..` traversal are rejected — a manifest may point
/// anywhere in ITS OWN source tree, not at arbitrary hosts.
fn resolve_item_url(manifest_url: &str, path: &str) -> Result<String, String> {
    let path = path.trim().trim_start_matches('/');
    if path.is_empty() {
        return Err("item path is empty".into());
    }
    if path.contains("://") {
        return Err("item path must be relative to the source".into());
    }
    if path.split('/').any(|seg| seg == "..") {
        return Err("item path must not traverse upward".into());
    }
    let base = manifest_url
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or(manifest_url);
    Ok(format!("{base}/{path}"))
}

fn parse_manifest(bytes: &[u8]) -> Result<MarketManifest, String> {
    let manifest: MarketManifest =
        serde_json::from_slice(bytes).map_err(|e| format!("index.json is not a valid v1 manifest: {e}"))?;
    if manifest.version != 1 {
        return Err(format!("unsupported manifest version {}", manifest.version));
    }
    Ok(manifest)
}

impl DevopsService {
    // -- market sources (P1-1 round 2) -----------------------------------

    pub async fn list_market_sources(&self, tenant_id: &str) -> Result<Vec<MarketSourceDto>, DevopsError> {
        let rows: Vec<SourceRow> = self.db.fetch_all_as::<SourceRow>(
            "SELECT id, tenant_id, name, url, enabled, last_synced_at, last_sync_status, last_sync_error, \
                    created_by, created_at, updated_at \
             FROM one_market_sources WHERE tenant_id = ? ORDER BY created_at DESC",
        &db_params![tenant_id])
        .await?;
        Ok(rows.into_iter().map(row_to_source).collect())
    }

    async fn get_market_source(&self, tenant_id: &str, id: &str) -> Result<SourceRow, DevopsError> {
        self.db
            .fetch_optional_as::<SourceRow>(
                "SELECT id, tenant_id, name, url, enabled, last_synced_at, last_sync_status, last_sync_error, \
                    created_by, created_at, updated_at \
             FROM one_market_sources WHERE tenant_id = ? AND id = ?",
                &db_params![tenant_id, id],
            )
            .await?
            .ok_or(DevopsError::NotFound("market source not found".into()))
    }

    pub async fn create_market_source(
        &self,
        tenant_id: &str,
        name: &str,
        url: &str,
        created_by: &str,
    ) -> Result<MarketSourceDto, DevopsError> {
        let name = name.trim();
        let url = url.trim();
        if name.is_empty() {
            return Err(DevopsError::BadRequest("source name is required".into()));
        }
        let scheme = url.split("://").next().unwrap_or_default().to_lowercase();
        if !matches!(scheme.as_str(), "http" | "https") || !url.contains("://") {
            return Err(DevopsError::BadRequest(
                "source url must be an http(s) URL pointing at an index.json".into(),
            ));
        }
        let id = new_id("msrc");
        let now = now_ms();
        self.db.execute(
            "INSERT INTO one_market_sources (id, tenant_id, name, url, enabled, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 1, ?, ?, ?)",
        &db_params![&id, tenant_id, name, url, created_by, now, now])
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                DevopsError::BadRequest("a source with this name already exists".into())
            } else {
                DevopsError::from(e)
            }
        })?;
        Ok(row_to_source(self.get_market_source(tenant_id, &id).await?))
    }

    pub async fn update_market_source(
        &self,
        tenant_id: &str,
        id: &str,
        name: Option<&str>,
        url: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<MarketSourceDto, DevopsError> {
        let existing = self.get_market_source(tenant_id, id).await?;
        let name = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&existing.2);
        let url = url.map(str::trim).filter(|u| !u.is_empty()).unwrap_or(&existing.3);
        let enabled = enabled.unwrap_or(existing.4);
        self.db.execute("UPDATE one_market_sources SET name = ?, url = ?, enabled = ?, updated_at = ? WHERE id = ?", &db_params![name, url, enabled, now_ms(), id])
            .await?;
        Ok(row_to_source(self.get_market_source(tenant_id, id).await?))
    }

    /// Deletes the source and its import mapping; registry rows imported
    /// from it are KEPT — they are published content now, and removing a
    /// source must not silently retract content members already see.
    pub async fn delete_market_source(&self, tenant_id: &str, id: &str) -> Result<(), DevopsError> {
        self.get_market_source(tenant_id, id).await?;
        let mut tx = self.db.begin().await?;
        tx.execute("DELETE FROM one_market_imports WHERE source_id = ?", &db_params![id])
            .await?;
        tx.execute("DELETE FROM one_market_sources WHERE id = ?", &db_params![id])
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Sync one source against its manifest. Synchronous and idempotent —
    /// the codebase adds no background schedulers, and an admin pressing
    /// "sync" wants the report, not a promise.
    pub async fn sync_market_source(
        &self,
        tenant_id: &str,
        source_id: &str,
        fetcher: &dyn MarketFetcher,
    ) -> Result<MarketSyncReportDto, DevopsError> {
        let (source_id, _tenant, _name, source_url, enabled, _synced, _status, _err, _by, _created, _updated) =
            self.get_market_source(tenant_id, source_id).await?;
        if !enabled {
            return Err(DevopsError::BadRequest("source is disabled".into()));
        }

        let mut report = MarketSyncReportDto {
            source_id: source_id.clone(),
            total_indexed: 0,
            imported: 0,
            updated: 0,
            skipped: 0,
            removed: Vec::new(),
            errors: Vec::new(),
            synced_at: now_ms(),
        };

        // Local alias so both call sites read the same; the update itself is
        // `record_market_sync_result` (a closure borrowing &self cannot name
        // its async future's lifetime cleanly).

        let index_bytes = match fetcher.fetch(&source_url, FETCH_CAP_BYTES).await {
            Ok(bytes) => bytes,
            Err(e) => {
                self.record_market_sync_result(&source_id, "error", Some(format!("index fetch failed: {e}")))
                    .await;
                return Err(DevopsError::BadRequest(format!("index fetch failed: {e}")));
            }
        };
        let manifest = match parse_manifest(&index_bytes) {
            Ok(m) => m,
            Err(e) => {
                self.record_market_sync_result(&source_id, "error", Some(e.clone()))
                    .await;
                return Err(DevopsError::BadRequest(e));
            }
        };
        report.total_indexed = manifest.items.len();

        for item in &manifest.items {
            let label = format!("{}:{}", item.kind, item.name);
            if !MARKET_KINDS.contains(&item.kind.as_str()) {
                report.errors.push(MarketSyncItemError {
                    item: label,
                    error: format!("unknown kind '{}'", item.kind),
                });
                continue;
            }
            if item.name.trim().is_empty() {
                report.errors.push(MarketSyncItemError {
                    item: label,
                    error: "item name is empty".into(),
                });
                continue;
            }
            if let Err(e) = self
                .sync_market_item(tenant_id, &source_id, &source_url, item, fetcher, &mut report)
                .await
            {
                report.errors.push(MarketSyncItemError { item: label, error: e });
            }
        }

        // Items this source previously imported that are gone from the
        // upstream index: reported, rows kept (see the migration's doc).
        let known: Vec<(String, String)> =
            self.db.fetch_all_as::<(String, String)>("SELECT kind, item_name FROM one_market_imports WHERE source_id = ?", &db_params![&source_id])
                .await?;
        for (kind, name) in known {
            let still_listed = manifest.items.iter().any(|item| item.kind == kind && item.name == name);
            if !still_listed {
                report.removed.push(format!("{kind}:{name}"));
            }
        }

        self.record_market_sync_result(&source_id, "ok", None).await;
        Ok(report)
    }

    /// One manifest item: fetch (skills) / validate / hash / upsert. Errors
    /// are the ITEM's, reported and isolated.
    async fn sync_market_item(
        &self,
        // Registry rows carry no tenant column (tenancy is scope-based, see
        // 013_content_origin.sql); the parameter exists only for signature
        // symmetry with the caller and stays unused.
        _tenant_id: &str,
        source_id: &str,
        manifest_url: &str,
        item: &ManifestItem,
        fetcher: &dyn MarketFetcher,
        report: &mut MarketSyncReportDto,
    ) -> Result<(), String> {
        let mapping: Option<(String, String)> = self.db.fetch_optional_as::<(String, String)>(
            "SELECT registry_id, content_hash FROM one_market_imports WHERE source_id = ? AND kind = ? AND item_name = ?",
        &db_params![source_id, &item.kind, &item.name])
        .await
        .map_err(|e| format!("mapping lookup failed: {e}"))?;

        // A publisher-pinned hash matching the recorded one skips the item
        // entirely — no fetch, no write. Everything below is for new or
        // changed items.
        if let (Some((_, existing_hash)), Some(declared)) = (&mapping, &item.sha256) {
            if declared.eq_ignore_ascii_case(existing_hash) {
                report.skipped += 1;
                return Ok(());
            }
        }

        let (payload_hash, import) = match item.kind.as_str() {
            "skill" => {
                let path = item
                    .path
                    .as_deref()
                    .ok_or("skill item requires a 'path' to its SKILL.md")?;
                let file_url = resolve_item_url(manifest_url, path)?;
                let bytes = fetcher.fetch(&file_url, FETCH_CAP_BYTES).await?;
                let content = String::from_utf8(bytes).map_err(|_| "SKILL.md must be UTF-8 text".to_string())?;
                let parsed = dream_core_cron::skill_file::validate_skill_content(&content)
                    .map_err(|e| format!("SKILL.md failed validation: {e}"))?;
                if parsed.name != item.name {
                    return Err(format!(
                        "manifest name '{}' does not match the SKILL.md frontmatter name '{}'",
                        item.name, parsed.name
                    ));
                }
                (
                    sha256_hex(content.as_bytes()),
                    MarketImport::Skill {
                        name: parsed.name,
                        description: parsed.description,
                        content,
                        category_id: item.category.clone(),
                    },
                )
            }
            "mcp" => {
                let mcp = item
                    .mcp
                    .as_ref()
                    .ok_or("mcp item requires an 'mcp' object with type and endpoint")?;
                if !matches!(mcp.r#type.as_str(), "stdio" | "sse") {
                    return Err(format!("mcp type must be 'stdio' or 'sse', got '{}'", mcp.r#type));
                }
                let canonical = serde_json::json!({ "type": mcp.r#type, "endpoint": mcp.endpoint });
                (
                    sha256_hex(canonical.to_string().as_bytes()),
                    MarketImport::Mcp {
                        name: item.name.clone(),
                        mcp_type: mcp.r#type.clone(),
                        endpoint: mcp.endpoint.clone(),
                        category_id: item.category.clone(),
                    },
                )
            }
            other => return Err(format!("unknown kind '{other}'")),
        };

        if let Some(declared) = &item.sha256 {
            if !payload_hash.eq_ignore_ascii_case(declared) {
                return Err(format!(
                    "fetched payload hash {payload_hash} does not match the manifest pin {declared}"
                ));
            }
        }

        if let Some((registry_id, existing_hash)) = mapping {
            if existing_hash == payload_hash {
                report.skipped += 1;
                return Ok(());
            }
            import.update_by_id(&self.db, &registry_id).await?;
            self.db.execute("UPDATE one_market_imports SET content_hash = ?, updated_at = ? WHERE source_id = ? AND kind = ? AND item_name = ?", &db_params![&payload_hash, now_ms(), source_id, &item.kind, &item.name])
                .await
                .map_err(|e| format!("mapping update failed: {e}"))?;
            report.updated += 1;
            return Ok(());
        }

        // New mapping. Names are unique across the registry (D7, enforced at
        // the app level — no DB UNIQUE on those tables): a collision with a
        // self-built row is an error, never a takeover; a collision with
        // another MARKET row is an adoption (update in place + mapping
        // created), which also makes delete-source-then-re-add idempotent
        // instead of piling up orphan rows.
        let new_id = import.upsert_new(&self.db, &item.name, &item.kind).await?;
        self.db.execute(
            "INSERT INTO one_market_imports (source_id, kind, item_name, registry_id, content_hash, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        &db_params![source_id, &item.kind, &item.name, &new_id, &payload_hash, now_ms()])
        .await
        .map_err(|e| format!("mapping insert failed: {e}"))?;
        report.imported += 1;
        Ok(())
    }

    /// Persist the sync outcome on the source row (status + error text).
    /// Best-effort by the same convention as every telemetry write here: a
    /// failed status update is logged, never propagated — the report the
    /// caller holds is the authoritative result.
    async fn record_market_sync_result(&self, source_id: &str, status: &str, error: Option<String>) {
        if let Err(e) = self.db.execute(
            "UPDATE one_market_sources SET last_synced_at = ?, last_sync_status = ?, last_sync_error = ? WHERE id = ?",
            &db_params![now_ms(), status, error, source_id],
        )
        .await
        {
            tracing::warn!(error = %e, source_id, "failed to record the market sync result");
        }
    }
}

/// The registry write for one manifest item, split by kind. Both variants
/// write origin='market' and published=1 on INSERT — a market import is
/// published by definition (the admin synced it to distribute); UPDATE
/// paths keep whatever publish state the tenant set afterwards.
enum MarketImport {
    Skill {
        name: String,
        description: String,
        content: String,
        category_id: Option<String>,
    },
    Mcp {
        name: String,
        mcp_type: String,
        endpoint: String,
        category_id: Option<String>,
    },
}

impl MarketImport {
    async fn update_by_id(&self, pool: &DbPool, registry_id: &str) -> Result<(), String> {
        match self {
            Self::Skill {
                description,
                content,
                category_id,
                ..
            } => {
                pool.execute(
                    "UPDATE one_skill_registry SET description = ?, content = ?, category_id = ?, updated_at = ? WHERE id = ?",
                    &db_params![description, content, category_id, now_ms(), registry_id],
                )
                .await
                .map_err(|e| format!("registry update failed: {e}"))?;
            }
            Self::Mcp {
                mcp_type,
                endpoint,
                category_id,
                ..
            } => {
                pool.execute(
                    "UPDATE one_mcp_registry SET type = ?, endpoint = ?, category_id = ?, updated_at = ? WHERE id = ?",
                    &db_params![mcp_type, endpoint, category_id, now_ms(), registry_id],
                )
                .await
                .map_err(|e| format!("registry update failed: {e}"))?;
            }
        }
        Ok(())
    }

    /// Insert (no existing row) or adopt (an existing origin='market' row
    /// with this name), and return the registry id. A self-built row with
    /// the same name is an error — admin-authored content is never taken
    /// over by a sync.
    async fn upsert_new(&self, pool: &DbPool, item_name: &str, kind: &str) -> Result<String, String> {
        let table = if kind == "skill" {
            "one_skill_registry"
        } else {
            "one_mcp_registry"
        };
        let existing: Option<(String, String)> =
            pool.fetch_optional_as::<(String, String)>(&format!("SELECT id, origin FROM {table} WHERE name = ?"), &db_params![item_name])
                .await
                .map_err(|e| format!("collision check failed: {e}"))?;
        if let Some((id, origin)) = existing {
            if origin != "market" {
                return Err(format!(
                    "name '{item_name}' is already taken by a self-built row; rename one of them"
                ));
            }
            // Adopt: another source (or a deleted-and-re-added one) owns this
            // market row. Update it into this source's shape.
            self.update_by_id(pool, &id).await?;
            return Ok(id);
        }
        let now = now_ms();
        match self {
            Self::Skill {
                name,
                description,
                content,
                category_id,
            } => {
                let id = new_id("oskill");
                pool.execute(
                    "INSERT INTO one_skill_registry \
                     (id, name, description, content, enabled, auto_active, scope, team_id, visibility, \
                      origin, category_id, published, created_by, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, 1, 0, 'org', NULL, 'all', 'market', ?, 1, 'market-sync', ?, ?)",
                &db_params![&id, name, description, content, category_id, now, now])
                .await
                .map_err(|e| format!("registry insert failed: {e}"))?;
                Ok(id)
            }
            Self::Mcp {
                name,
                mcp_type,
                endpoint,
                category_id,
            } => {
                let id = new_id("omcp");
                pool.execute(
                    "INSERT INTO one_mcp_registry \
                     (id, name, type, endpoint, enabled, has_keys, secrets_json, scope, team_id, visibility, \
                      origin, category_id, published, created_by, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, 1, 0, NULL, 'org', NULL, 'all', 'market', ?, 1, 'market-sync', ?, ?)",
                &db_params![&id, name, mcp_type, endpoint, category_id, now, now])
                .await
                .map_err(|e| format!("registry insert failed: {e}"))?;
                Ok(id)
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    async fn setup() -> (sqlx::SqlitePool, DevopsService) {
        // Single connection so the in-memory database outlives one call.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::migrate::run_one_devops_migrations(&dream_core_db::DbPool::Sqlite(pool.clone())).await.unwrap();
        let service = DevopsService::new(dream_core_db::DbPool::Sqlite(pool.clone()));
        (pool, service)
    }

    /// Serves canned URLs; anything else is a 404-style error.
    struct FakeFetcher {
        responses: HashMap<String, &'static [u8]>,
        fetch_log: Mutex<Vec<String>>,
    }

    impl FakeFetcher {
        fn new(pairs: &[(&str, &'static [u8])]) -> Self {
            Self {
                responses: pairs.iter().map(|(u, b)| (u.to_string(), *b)).collect(),
                fetch_log: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl MarketFetcher for FakeFetcher {
        async fn fetch(&self, url: &str, _max: usize) -> Result<Vec<u8>, String> {
            self.fetch_log.lock().unwrap().push(url.to_owned());
            self.responses
                .get(url)
                .map(|b| b.to_vec())
                .ok_or_else(|| format!("HTTP 404 for {url}"))
        }
    }

    const MANIFEST: &str = r#"{"version":1,"items":[
        {"kind":"skill","name":"review-skill","path":"skills/review/SKILL.md"},
        {"kind":"mcp","name":"jira","mcp":{"type":"sse","endpoint":"https://jira.example.com/mcp"}}
    ]}"#;

    const SKILL_MD: &str = "---\nname: review-skill\ndescription: Reviews code\n---\n\nDo a review.";

    fn source_url() -> &'static str {
        "https://market.example.com/base/index.json"
    }

    fn skill_url() -> &'static str {
        "https://market.example.com/base/skills/review/SKILL.md"
    }

    async fn make_source(service: &DevopsService) -> String {
        service
            .create_market_source("t1", "acme", source_url(), "admin1")
            .await
            .unwrap()
            .id
    }

    async fn registry_row(pool: &sqlx::SqlitePool, table: &str, name: &str) -> Option<(String, String, i64)> {
        sqlx::query_as::<_, (String, String, i64)>(&format!("SELECT id, origin, published FROM {table} WHERE name = ?"))
            .bind(name)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn sync_imports_skills_and_mcp_with_market_origin() {
        let (pool, service) = setup().await;
        let id = make_source(&service).await;
        let fetcher = FakeFetcher::new(&[(source_url(), MANIFEST.as_bytes()), (skill_url(), SKILL_MD.as_bytes())]);

        let report = service.sync_market_source("t1", &id, &fetcher).await.unwrap();
        assert_eq!(report.total_indexed, 2);
        assert_eq!(report.imported, 2);
        assert_eq!(report.errors.len(), 0);

        let (_, _, published) = registry_row(&pool, "one_skill_registry", "review-skill")
            .await
            .expect("skill imported");
        assert_eq!(published, 1);
        let (_, mcp_origin, _) = registry_row(&pool, "one_mcp_registry", "jira")
            .await
            .expect("mcp imported");
        assert_eq!(mcp_origin, "market");

        let count: i64 = dream_core_db::DbPool::Sqlite(pool.clone()).fetch_one_scalar("SELECT COUNT(*) FROM one_market_imports WHERE source_id = ?", &db_params![&id])
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn resync_with_a_matching_pin_skips_the_fetch_entirely() {
        let (pool, service) = setup().await;
        let id = make_source(&service).await;
        let fetcher = FakeFetcher::new(&[(source_url(), MANIFEST.as_bytes()), (skill_url(), SKILL_MD.as_bytes())]);
        service.sync_market_source("t1", &id, &fetcher).await.unwrap();

        // Second round: the manifest pins the content hashes, so the sync
        // skips BOTH items without fetching either payload — the index is
        // the only request.
        let pinned = format!(
            r#"{{"version":1,"items":[
            {{"kind":"skill","name":"review-skill","path":"skills/review/SKILL.md","sha256":"{skill_hash}"}},
            {{"kind":"mcp","name":"jira","sha256":"{mcp_hash}","mcp":{{"type":"sse","endpoint":"https://jira.example.com/mcp"}}}}
        ]}}"#,
            skill_hash = sha256_hex(SKILL_MD.as_bytes()),
            mcp_hash = sha256_hex(
                serde_json::json!({"type":"sse","endpoint":"https://jira.example.com/mcp"})
                    .to_string()
                    .as_bytes()
            ),
        );
        let pinned: &'static [u8] = Box::leak(pinned.into_bytes().into_boxed_slice());
        let fetcher2 = FakeFetcher::new(&[(source_url(), pinned)]);
        let report = service.sync_market_source("t1", &id, &fetcher2).await.unwrap();
        assert_eq!(report.imported, 0);
        assert_eq!(report.updated, 0);
        assert_eq!(report.skipped, 2, "matching pins skip without a fetch or a write");
        assert_eq!(
            fetcher2.fetch_log.lock().unwrap().len(),
            1,
            "only the index was fetched; payloads were skipped"
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn a_pin_disagreeing_with_the_fetched_payload_is_an_integrity_error() {
        let (pool, service) = setup().await;
        let id = make_source(&service).await;
        let wrong_pin = "0".repeat(64);
        let pinned = format!(
            r#"{{"version":1,"items":[
            {{"kind":"skill","name":"review-skill","path":"skills/review/SKILL.md","sha256":"{wrong_pin}"}}
        ]}}"#
        );
        let pinned: &'static [u8] = Box::leak(pinned.into_bytes().into_boxed_slice());
        let fetcher = FakeFetcher::new(&[(source_url(), pinned), (skill_url(), SKILL_MD.as_bytes())]);

        let report = service.sync_market_source("t1", &id, &fetcher).await.unwrap();
        assert_eq!(report.imported, 0, "a tampered payload must not land");
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].error.contains("does not match the manifest pin"));
        pool.close().await;
    }

    #[tokio::test]
    async fn changed_content_updates_the_registry_row() {
        let (pool, service) = setup().await;
        let id = make_source(&service).await;
        let fetcher = FakeFetcher::new(&[(source_url(), MANIFEST.as_bytes()), (skill_url(), SKILL_MD.as_bytes())]);
        service.sync_market_source("t1", &id, &fetcher).await.unwrap();

        let changed: &'static [u8] = b"---\nname: review-skill\ndescription: Reviews code v2\n---\n\nUpdated body.";
        let fetcher2 = FakeFetcher::new(&[(source_url(), MANIFEST.as_bytes()), (skill_url(), changed)]);
        let report = service.sync_market_source("t1", &id, &fetcher2).await.unwrap();
        assert_eq!(report.updated, 1);
        assert_eq!(report.imported, 0);

        let row: (String,) = sqlx::query_as("SELECT content FROM one_skill_registry WHERE name = 'review-skill'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(row.0.contains("Updated body."));
    }

    #[tokio::test]
    async fn a_self_built_name_collision_is_an_item_error_not_a_takeover() {
        let (pool, service) = setup().await;
        service
            .upsert_skill(
                None,
                "review-skill",
                "admin's own",
                "own content",
                true,
                false,
                "org",
                None,
                "all",
                None,
                "admin1",
            )
            .await
            .unwrap();
        let id = make_source(&service).await;
        let fetcher = FakeFetcher::new(&[(source_url(), MANIFEST.as_bytes()), (skill_url(), SKILL_MD.as_bytes())]);

        let report = service.sync_market_source("t1", &id, &fetcher).await.unwrap();
        assert_eq!(report.imported, 1, "the MCP item still imports (partial success)");
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].error.contains("self-built"));
        let origin: String = dream_core_db::DbPool::Sqlite(pool.clone())
            .fetch_one_scalar(
                "SELECT origin FROM one_skill_registry WHERE name = 'review-skill'",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(origin, "self_built");
    }

    #[tokio::test]
    async fn items_missing_from_upstream_are_reported_and_rows_kept() {
        let (pool, service) = setup().await;
        let id = make_source(&service).await;
        let fetcher = FakeFetcher::new(&[(source_url(), MANIFEST.as_bytes()), (skill_url(), SKILL_MD.as_bytes())]);
        service.sync_market_source("t1", &id, &fetcher).await.unwrap();

        let mcp_only: &'static [u8] =
            br#"{"version":1,"items":[{"kind":"mcp","name":"jira","mcp":{"type":"sse","endpoint":"https://jira.example.com/mcp"}}]}"#;
        let fetcher2 = FakeFetcher::new(&[(source_url(), mcp_only)]);
        let report = service.sync_market_source("t1", &id, &fetcher2).await.unwrap();
        assert_eq!(report.removed, vec!["skill:review-skill".to_owned()]);
        assert!(
            registry_row(&pool, "one_skill_registry", "review-skill")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn source_level_failures_record_error_status() {
        let (pool, service) = setup().await;
        let id = make_source(&service).await;
        let fetcher = FakeFetcher::new(&[]);

        let err = service.sync_market_source("t1", &id, &fetcher).await.unwrap_err();
        assert!(err.to_string().contains("index fetch failed"));

        let source = service.list_market_sources("t1").await.unwrap().remove(0);
        assert_eq!(source.last_sync_status.as_deref(), Some("error"));
        assert!(
            source
                .last_sync_error
                .as_deref()
                .unwrap()
                .contains("index fetch failed")
        );

        let bad: &'static [u8] = br#"{"version":9,"items":[]}"#;
        let fetcher2 = FakeFetcher::new(&[(source_url(), bad)]);
        let err2 = service.sync_market_source("t1", &id, &fetcher2).await.unwrap_err();
        assert!(err2.to_string().contains("unsupported manifest version"));
    }

    #[tokio::test]
    async fn disabled_sources_refuse_to_sync_and_delete_keeps_registry_rows() {
        let (pool, service) = setup().await;
        let id = make_source(&service).await;
        let fetcher = FakeFetcher::new(&[(source_url(), MANIFEST.as_bytes()), (skill_url(), SKILL_MD.as_bytes())]);
        service.sync_market_source("t1", &id, &fetcher).await.unwrap();

        service
            .update_market_source("t1", &id, None, None, Some(false))
            .await
            .unwrap();
        assert_eq!(
            service
                .sync_market_source("t1", &id, &fetcher)
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );

        service.delete_market_source("t1", &id).await.unwrap();
        assert!(service.list_market_sources("t1").await.unwrap().is_empty());
        let mappings: i64 = dream_core_db::DbPool::Sqlite(pool.clone()).fetch_one_scalar("SELECT COUNT(*) FROM one_market_imports", &[])
            .await
            .unwrap();
        assert_eq!(mappings, 0);
        assert!(
            registry_row(&pool, "one_skill_registry", "review-skill")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn source_validation_and_tenant_isolation() {
        let (pool, service) = setup().await;
        assert_eq!(
            service
                .create_market_source("t1", "x", "ftp://example.com/index.json", "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service
                .create_market_source("t1", "  ", "https://a.example.com/index.json", "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        service
            .create_market_source("t1", "acme", source_url(), "admin1")
            .await
            .unwrap();
        assert_eq!(
            service
                .create_market_source("t1", "acme", "https://b.example.com/index.json", "admin1")
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        service
            .create_market_source("t2", "other-tenant", source_url(), "admin2")
            .await
            .unwrap();
        assert_eq!(service.list_market_sources("t1").await.unwrap().len(), 1);
    }
}
