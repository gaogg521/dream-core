//! API assets (P1-6, align-openocta): imported Swagger / OpenAPI documents.
//!
//! Two halves, both real:
//!
//! 1. **Import + browse.** A JSON OpenAPI/Swagger document is stored verbatim
//!    (`spec`, for replay/audit) alongside a parsed endpoint summary
//!    (`endpoints`) the UI renders without re-parsing. Parsing is a
//!    hand-rolled `serde_json` walk over `paths` — deliberately no
//!    swagger/openapi crate. YAML is a known limitation: only JSON is
//!    accepted (the workspace carries no serde_yaml dependency, so a YAML
//!    payload cannot even be deserialized here; the client converts first).
//!
//! 2. **Expose to agents.** This is NOT a fake "tool registration": devops
//!    skill-registry rows carry full SKILL.md `content`, and the verified
//!    materialization chain is — member client fetches registry rows visible
//!    to it → `POST /api/skills/team-sync` (`dream-core-extension::team_sync`)
//!    → writes a real `{data_dir}/team-skills/{id}/SKILL.md` (frontmatter
//!    trusted when the content starts with `---`) → skill loader lists it
//!    (source=Team, `.team-auto` marker = auto-active) → `AcpSkillManager`
//!    injects the skill into agent context. Agents then call the endpoints
//!    *for real* through their terminal tools (curl) following the skill
//!    body. So "publish as skill" generates that SKILL.md from the parsed
//!    endpoints and writes it through the ordinary `upsert_skill` path.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

use dream_core_common::now_ms;

use crate::error::DevopsError;
use crate::models::SkillRegistryDto;
use crate::service::{DevopsService, new_id};
use dream_core_db::db_params;

/// One parsed endpoint summary. Method/path casing is kept exactly as it
/// appears in the document; unparseable optional fields become `None` rather
/// than failing the import (the raw `spec` is the source of truth anyway).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiEndpoint {
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
struct ApiAssetRow {
    id: String,
    #[allow(dead_code)]
    tenant_id: String,
    name: String,
    source_format: String,
    title: Option<String>,
    version: Option<String>,
    base_url: Option<String>,
    spec: String,
    endpoints: String,
    imported_by: String,
    published_skill_id: Option<String>,
    created_at: i64,
    updated_at: i64,
    // -- 017 (manual authoring) ----------------------------------------
    code: Option<String>,
    category: Option<String>,
    tool_prefix: Option<String>,
    auth_type: String,
    /// Credentials. Read into the row so `into_dto` can report *whether* an
    /// asset is authenticated; it is dropped there and must never be copied
    /// into a DTO, a log line, or an audit payload.
    auth_config: Option<String>,
    /// SQLite has no bool; MySQL stores TINYINT(1). Both decode as an integer.
    enabled: i64,
    description: Option<String>,
}

/// An asset as the UI list/browse renders it. `spec` is deliberately absent
/// from the list payload (documents can be large); fetch the detail endpoint.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiAssetDto {
    pub id: String,
    pub name: String,
    /// 'openapi' | 'swagger'
    pub source_format: String,
    pub title: Option<String>,
    pub version: Option<String>,
    pub base_url: Option<String>,
    /// Parsed endpoint summary array (already JSON, ready for the UI).
    pub endpoints: Value,
    pub endpoint_count: usize,
    /// Set once the asset has been published into the skill registry.
    pub published_skill_id: Option<String>,
    pub imported_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Operator-facing handle, unique per tenant. `None` for the documents
    /// imported before 017 — they were only ever addressed by id.
    pub code: Option<String>,
    pub category: Option<String>,
    /// Prefix the published operations are named with. `None` means "use
    /// `code`"; the UI states that rather than pre-filling it.
    pub tool_prefix: Option<String>,
    /// 'none' | 'api_key' | 'bearer' | 'basic'
    pub auth_type: String,
    /// Whether credentials are stored — never the credentials themselves.
    pub auth_configured: bool,
    pub enabled: bool,
    pub description: Option<String>,
}

/// Detail variant: same as [`ApiAssetDto`] plus the raw stored document.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiAssetDetailDto {
    #[serde(flatten)]
    pub asset: ApiAssetDto,
    /// The original document, verbatim.
    pub spec: Value,
}

/// Fields an operator types by hand, on create and on edit alike.
///
/// Every field is optional so one struct serves both: on create the required
/// ones are checked explicitly, on edit `None` means "leave this alone". The
/// one field that needs a third state is `auth_config` — `None` keeps the
/// stored credentials, `Some("")` clears them, `Some(json)` replaces them.
/// Without that distinction an edit form that (correctly) never renders the
/// stored token would wipe it on every save.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiAssetProfileInput {
    pub name: Option<String>,
    pub code: Option<String>,
    pub category: Option<String>,
    pub tool_prefix: Option<String>,
    pub base_url: Option<String>,
    pub auth_type: Option<String>,
    pub auth_config: Option<String>,
    pub enabled: Option<bool>,
    pub description: Option<String>,
}

/// Authentication shapes the console offers. Kept as a closed set because
/// `publish_api_asset_skill` has to describe each one in the generated
/// SKILL.md — an unknown value would silently produce a skill that tells the
/// agent nothing about how to authenticate.
/// Trim, then treat an all-whitespace field as absent — the console posts
/// empty strings for untouched optional inputs.
fn trimmed(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

const AUTH_TYPES: [&str; 4] = ["none", "api_key", "bearer", "basic"];

/// A code is the tenant-unique handle *and* the default tool prefix, so it has
/// to survive being pasted into a tool name: lowercase, no spaces, no dots.
fn normalize_code(raw: &str) -> Result<String, DevopsError> {
    let code = raw.trim().to_ascii_lowercase();
    if code.is_empty() {
        return Err(DevopsError::BadRequest("code is required".into()));
    }
    if code.len() > 64 {
        return Err(DevopsError::BadRequest("code must be at most 64 characters".into()));
    }
    if !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(DevopsError::BadRequest(
            "code may only contain letters, digits and underscores".into(),
        ));
    }
    Ok(code)
}

/// Resolve one optional text field on edit.
///
/// Three cases, and the middle one is the bug this exists to prevent: the
/// console posts every optional input, so a field the operator left blank
/// arrives as `Some("")`. Storing that verbatim gives the column an empty
/// string where it should hold NULL — and `tool_prefix` in particular is read
/// as "unset means fall back to the code", which an empty string silently
/// defeats.
fn resolve_optional(input: Option<&str>, current: &Option<String>) -> Option<String> {
    match input {
        None => current.clone(),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() { None } else { Some(trimmed.to_owned()) }
        }
    }
}

fn validate_auth(auth_type: &str, auth_config: Option<&str>) -> Result<(), DevopsError> {
    if !AUTH_TYPES.contains(&auth_type) {
        return Err(DevopsError::BadRequest(format!(
            "unsupported auth type {auth_type}; expected one of {}",
            AUTH_TYPES.join(", ")
        )));
    }
    // An unparseable blob would be stored and then fail at call time, far away
    // from the form that produced it.
    if let Some(raw) = auth_config.map(str::trim).filter(|c| !c.is_empty()) {
        let parsed: Value =
            serde_json::from_str(raw).map_err(|e| DevopsError::BadRequest(format!("auth config must be JSON: {e}")))?;
        if !parsed.is_object() {
            return Err(DevopsError::BadRequest("auth config must be a JSON object".into()));
        }
    }
    Ok(())
}

/// Result of a strict parse of an imported document.
struct ParsedSpec {
    source_format: &'static str,
    title: Option<String>,
    version: Option<String>,
    base_url: Option<String>,
    endpoints: Vec<ApiEndpoint>,
}

/// HTTP method keys recognized inside a `paths` entry. Everything else under
/// a path item (parameters, summary, `$ref`, `x-*`) is ignored.
const PATH_ITEM_METHODS: [&str; 8] = ["get", "put", "post", "delete", "options", "head", "patch", "trace"];

/// Validate + parse an imported document. Tolerant on optional fields,
/// strict on the shape: the payload must be a JSON object that carries a
/// `paths` object and a recognizable version field, otherwise the import is
/// rejected with `BadRequest` (a typo'd document must not silently produce a
/// zero-endpoint asset).
fn parse_spec(spec: &Value) -> Result<ParsedSpec, DevopsError> {
    let obj = spec
        .as_object()
        .ok_or_else(|| DevopsError::BadRequest("spec must be a JSON object (OpenAPI/Swagger document)".into()))?;

    // Format detection: OpenAPI 3.x carries `openapi`, Swagger carries
    // `swagger: "2.0"`. (Swagger 2 docs also have `paths`, so the same
    // endpoint walk serves both.)
    let source_format: &'static str = match obj.get("openapi").and_then(Value::as_str) {
        Some(v) if v.starts_with('2') || v.starts_with('3') => "openapi",
        _ => match obj.get("swagger").and_then(Value::as_str) {
            Some(v) if v.starts_with('2') => "swagger",
            _ => {
                return Err(DevopsError::BadRequest(
                    "spec is neither OpenAPI nor Swagger: missing an 'openapi' or 'swagger' version field".into(),
                ));
            }
        },
    };

    let paths = obj
        .get("paths")
        .ok_or_else(|| DevopsError::BadRequest("spec has no 'paths' object".into()))?
        .as_object()
        .ok_or_else(|| DevopsError::BadRequest("spec 'paths' must be an object".into()))?;

    let info = obj.get("info");
    let title = info
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let version = info
        .and_then(|i| i.get("version"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    // Best-effort base URL: OpenAPI 3 `servers[0].url`, Swagger 2
    // `host` (+ optional `basePath`, `schemes[0]`). Absent → null.
    let base_url = obj
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(|s| s.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            let host = obj.get("host").and_then(Value::as_str)?;
            let scheme = obj
                .get("schemes")
                .and_then(Value::as_array)
                .and_then(|s| s.first())
                .and_then(Value::as_str)
                .unwrap_or("https");
            let base_path = obj.get("basePath").and_then(Value::as_str).unwrap_or("");
            Some(format!("{scheme}://{host}{base_path}"))
        });

    let mut endpoints = Vec::new();
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for method in PATH_ITEM_METHODS {
            let Some(op) = item.get(method) else {
                continue;
            };
            let op = op.as_object();
            endpoints.push(ApiEndpoint {
                method: method.to_owned(),
                path: path.clone(),
                summary: op
                    .and_then(|o| o.get("summary"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                operation_id: op
                    .and_then(|o| o.get("operationId"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }

    Ok(ParsedSpec {
        source_format,
        title,
        version,
        base_url,
        endpoints,
    })
}

const ASSET_COLS: &str = "id, tenant_id, name, source_format, title, version, base_url, spec, endpoints, \
                          imported_by, published_skill_id, created_at, updated_at, code, category, \
                          tool_prefix, auth_type, auth_config, enabled, description";

impl ApiAssetRow {
    fn into_dto(self) -> Result<ApiAssetDto, DevopsError> {
        let parsed: Vec<ApiEndpoint> = serde_json::from_str(&self.endpoints).unwrap_or_default();
        let endpoint_count = parsed.len();
        Ok(ApiAssetDto {
            endpoint_count,
            endpoints: serde_json::to_value(parsed).unwrap_or(Value::Array(Vec::new())),
            id: self.id,
            name: self.name,
            source_format: self.source_format,
            title: self.title,
            version: self.version,
            base_url: self.base_url,
            published_skill_id: self.published_skill_id,
            imported_by: self.imported_by,
            created_at: self.created_at,
            updated_at: self.updated_at,
            code: self.code,
            category: self.category,
            tool_prefix: self.tool_prefix,
            auth_type: self.auth_type,
            // The value itself stops here, on purpose.
            auth_configured: self.auth_config.as_deref().is_some_and(|c| !c.trim().is_empty()),
            enabled: self.enabled != 0,
            description: self.description,
        })
    }
}

impl DevopsService {
    // -- API assets (P1-6) -------------------------------------------------

    /// Non-deleted assets of one tenant, newest first.
    ///
    /// `created_at` is milliseconds, so two imports in the same tick tie — and
    /// a tie under a bare `ORDER BY created_at DESC` is resolved by SQLite
    /// however it likes (in practice rowid ascending, i.e. OLDEST first, the
    /// exact opposite of the contract). `id` breaks it correctly rather than
    /// merely deterministically: these are UUIDv7, whose leading 48 bits are
    /// that same millisecond and whose counter keeps rising within it, so
    /// `id DESC` is genuine creation order — see `new_id`.
    pub async fn list_api_assets(&self, tenant_id: &str) -> Result<Vec<ApiAssetDto>, DevopsError> {
        let sql = format!(
            "SELECT {ASSET_COLS} FROM one_api_assets \
                           WHERE tenant_id = ? AND deleted_at IS NULL ORDER BY created_at DESC, id DESC"
        );
        let rows = self
            .db
            .fetch_all_as::<ApiAssetRow>(&sql, &db_params![tenant_id])
            .await?;
        rows.into_iter().map(ApiAssetRow::into_dto).collect()
    }

    /// One asset including the raw stored document.
    pub async fn get_api_asset(&self, tenant_id: &str, id: &str) -> Result<ApiAssetDetailDto, DevopsError> {
        let sql =
            format!("SELECT {ASSET_COLS} FROM one_api_assets WHERE id = ? AND tenant_id = ? AND deleted_at IS NULL");
        let row = self
            .db
            .fetch_optional_as::<ApiAssetRow>(&sql, &db_params![id, tenant_id])
            .await?
            .ok_or_else(|| DevopsError::NotFound(format!("api asset {id}")))?;
        let spec = row.spec.clone();
        Ok(ApiAssetDetailDto {
            asset: row.into_dto()?,
            spec: serde_json::from_str(&spec).unwrap_or(Value::String(spec)),
        })
    }

    /// Import a JSON OpenAPI/Swagger document. The document is validated and
    /// parsed up front; both the verbatim document and the parsed summary are
    /// stored in one row.
    pub async fn import_api_asset(
        &self,
        tenant_id: &str,
        imported_by: &str,
        name: &str,
        spec: &Value,
    ) -> Result<ApiAssetDto, DevopsError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DevopsError::BadRequest("name is required".into()));
        }
        let parsed = parse_spec(spec)?;

        let id = new_id("oapi");
        let now = now_ms();
        self.db
            .execute(
                "INSERT INTO one_api_assets \
                (id, tenant_id, name, source_format, title, version, base_url, spec, endpoints, imported_by, \
                 created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &db_params![
                    &id,
                    tenant_id,
                    name,
                    parsed.source_format,
                    &parsed.title,
                    &parsed.version,
                    &parsed.base_url,
                    spec.to_string(),
                    serde_json::to_string(&parsed.endpoints).map_err(|e| DevopsError::Internal(e.to_string()))?,
                    imported_by,
                    now,
                    now
                ],
            )
            .await?;

        self.get_api_asset(tenant_id, &id).await.map(|d| d.asset)
    }

    /// Create an asset by hand — the entry point for a service that has no
    /// published OpenAPI document. It carries no spec, so `source_format` is
    /// 'manual' and `endpoints` starts empty: operations are added afterwards
    /// on the detail page. `import_api_asset` remains the document path and is
    /// untouched.
    pub async fn create_api_asset(
        &self,
        tenant_id: &str,
        created_by: &str,
        input: &ApiAssetProfileInput,
    ) -> Result<ApiAssetDto, DevopsError> {
        let name = input.name.as_deref().unwrap_or_default().trim().to_owned();
        if name.is_empty() {
            return Err(DevopsError::BadRequest("name is required".into()));
        }
        let code = normalize_code(input.code.as_deref().unwrap_or_default())?;
        let auth_type = input.auth_type.as_deref().unwrap_or("none").trim().to_owned();
        validate_auth(&auth_type, input.auth_config.as_deref())?;
        self.ensure_code_free(tenant_id, &code, None).await?;

        let id = new_id("oapi");
        let now = now_ms();
        self.db
            .execute(
                "INSERT INTO one_api_assets                 (id, tenant_id, name, source_format, base_url, spec, endpoints, imported_by, created_at, updated_at,                  code, category, tool_prefix, auth_type, auth_config, enabled, description)              VALUES (?, ?, ?, 'manual', ?, '{}', '[]', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &db_params![
                    &id,
                    tenant_id,
                    &name,
                    trimmed(input.base_url.as_deref()),
                    created_by,
                    now,
                    now,
                    &code,
                    trimmed(input.category.as_deref()),
                    trimmed(input.tool_prefix.as_deref()),
                    &auth_type,
                    trimmed(input.auth_config.as_deref()),
                    i64::from(input.enabled.unwrap_or(true)),
                    trimmed(input.description.as_deref())
                ],
            )
            .await?;

        self.get_api_asset(tenant_id, &id).await.map(|d| d.asset)
    }

    /// Edit an asset's operator-authored fields. Read-modify-write rather than
    /// a dynamic SET clause: the column list is short, and one fixed statement
    /// is far easier to read than assembled SQL — the only cost is that two
    /// admins saving the same asset in the same second have last-write-wins,
    /// which this console already has everywhere else.
    pub async fn update_api_asset(
        &self,
        tenant_id: &str,
        id: &str,
        input: &ApiAssetProfileInput,
    ) -> Result<ApiAssetDto, DevopsError> {
        let current = self.get_api_asset(tenant_id, id).await?.asset;

        let name = match input.name.as_deref().map(str::trim) {
            Some(n) if n.is_empty() => return Err(DevopsError::BadRequest("name cannot be blank".into())),
            Some(n) => n.to_owned(),
            None => current.name.clone(),
        };
        let code = match input.code.as_deref() {
            Some(raw) => Some(normalize_code(raw)?),
            None => current.code.clone(),
        };
        if let Some(code) = code.as_deref() {
            self.ensure_code_free(tenant_id, code, Some(id)).await?;
        }
        let auth_type = input
            .auth_type
            .as_deref()
            .map(|a| a.trim().to_owned())
            .unwrap_or_else(|| current.auth_type.clone());
        validate_auth(&auth_type, input.auth_config.as_deref())?;

        // Three states in two bound values, so the statement and its
        // parameter count stay fixed: flag 0 keeps the stored credentials,
        // flag 1 writes `auth_value` (NULL when the operator cleared the
        // field). COALESCE cannot express this — it collapses "clear" and
        // "keep" into the same NULL.
        let (auth_set, auth_value): (i64, Option<String>) = match input.auth_config.as_deref().map(str::trim) {
            None => (0, None),
            Some("") => (1, None),
            Some(json) => (1, Some(json.to_owned())),
        };
        let sql = "UPDATE one_api_assets SET name = ?, code = ?, category = ?, tool_prefix = ?, base_url = ?, \
             auth_type = ?, enabled = ?, description = ?, \
             auth_config = CASE WHEN ? = 1 THEN ? ELSE auth_config END, updated_at = ? \
             WHERE id = ? AND tenant_id = ? AND deleted_at IS NULL";
        let params = db_params![
            &name,
            &code,
            resolve_optional(input.category.as_deref(), &current.category),
            resolve_optional(input.tool_prefix.as_deref(), &current.tool_prefix),
            resolve_optional(input.base_url.as_deref(), &current.base_url),
            &auth_type,
            i64::from(input.enabled.unwrap_or(current.enabled)),
            resolve_optional(input.description.as_deref(), &current.description),
            auth_set,
            auth_value,
            now_ms(),
            id,
            tenant_id
        ];

        let affected = self.db.execute(sql, &params).await?;
        if affected == 0 {
            return Err(DevopsError::NotFound(format!("api asset {id}")));
        }
        self.get_api_asset(tenant_id, id).await.map(|d| d.asset)
    }

    /// Guard the `(tenant_id, code)` uniqueness before writing, so a clash
    /// answers 400 with the offending code instead of surfacing a raw driver
    /// error. `skip_id` exempts the row being edited from colliding with
    /// itself. Soft-deleted rows still hold their code on MySQL (no partial
    /// index), so they are checked too and the message says as much.
    async fn ensure_code_free(&self, tenant_id: &str, code: &str, skip_id: Option<&str>) -> Result<(), DevopsError> {
        let existing = self
            .db
            .fetch_optional_as::<(String,)>(
                "SELECT id FROM one_api_assets WHERE tenant_id = ? AND code = ?",
                &db_params![tenant_id, code],
            )
            .await?;
        match existing {
            Some((found,)) if Some(found.as_str()) != skip_id => Err(DevopsError::BadRequest(format!(
                "asset code {code} is already used in this tenant"
            ))),
            _ => Ok(()),
        }
    }

    /// Soft delete (the row keeps its spec for audit; it just leaves every
    /// listing). Tenant-scoped like every other read.
    pub async fn delete_api_asset(&self, tenant_id: &str, id: &str) -> Result<(), DevopsError> {
        let result = self
            .db
            .execute(
                "UPDATE one_api_assets SET deleted_at = ? WHERE id = ? AND tenant_id = ? AND deleted_at IS NULL",
                &db_params![now_ms(), id, tenant_id],
            )
            .await?;
        if result == 0 {
            return Err(DevopsError::NotFound(format!("api asset {id}")));
        }
        Ok(())
    }

    /// Publish the asset's endpoints into the skill registry as an org-scope,
    /// member-visible skill. Re-publishing the same asset updates the original
    /// registry entry (the `published_skill_id` link), it never creates a
    /// second one. The registry's global name-uniqueness rule still applies:
    /// an asset whose name collides with an *unrelated* skill is rejected, not
    /// hijacked.
    ///
    /// `base_url` overrides the spec-detected one in the generated curl
    /// examples (e.g. pointing at a staging gateway); `None` falls back to the
    /// asset's own base_url, and with neither the examples read from a
    /// `$BASE_URL` shell variable.
    pub async fn publish_api_asset_skill(
        &self,
        tenant_id: &str,
        actor_user_id: &str,
        id: &str,
        base_url: Option<&str>,
        auto_active: bool,
    ) -> Result<SkillRegistryDto, DevopsError> {
        let sql =
            format!("SELECT {ASSET_COLS} FROM one_api_assets WHERE id = ? AND tenant_id = ? AND deleted_at IS NULL");
        let row = self
            .db
            .fetch_optional_as::<ApiAssetRow>(&sql, &db_params![id, tenant_id])
            .await?
            .ok_or_else(|| DevopsError::NotFound(format!("api asset {id}")))?;

        let endpoints: Vec<ApiEndpoint> = serde_json::from_str(&row.endpoints)
            .map_err(|e| DevopsError::Internal(format!("stored endpoints are not valid JSON: {e}")))?;
        let content = build_api_asset_skill_md(
            &row.name,
            row.title.as_deref(),
            row.version.as_deref(),
            base_url.or(row.base_url.as_deref()),
            &endpoints,
        );
        let description = format!(
            "API asset '{}' ({}): {} endpoint(s) an agent can call over HTTP with curl.",
            row.title.as_deref().unwrap_or(&row.name),
            row.source_format,
            endpoints.len()
        );

        let dto = self
            .upsert_skill(
                row.published_skill_id.as_deref(),
                &row.name,
                &description,
                &content,
                true,
                auto_active,
                "org",
                None,
                "all",
                None,
                actor_user_id,
            )
            .await?;

        if row.published_skill_id.as_deref() != Some(dto.id.as_str()) {
            self.db
                .execute(
                    "UPDATE one_api_assets SET published_skill_id = ?, updated_at = ? WHERE id = ?",
                    &db_params![&dto.id, now_ms(), &row.id],
                )
                .await?;
        }
        Ok(dto)
    }
}

/// Generate SKILL.md content (frontmatter + body) from the parsed endpoints.
///
/// The frontmatter is emitted unconditionally (name/description single-lined)
/// because `dream-core-extension::team_sync::build_skill_md` trusts a
/// `---`-prefixed payload as a complete file — this way the registry content
/// and what team-sync writes to disk are byte-identical.
fn build_api_asset_skill_md(
    name: &str,
    title: Option<&str>,
    version: Option<&str>,
    base_url: Option<&str>,
    endpoints: &[ApiEndpoint],
) -> String {
    let display = title.unwrap_or(name);
    let version_tag = version.map(|v| format!(" (v{v})")).unwrap_or_default();
    let description = format!("Call the {display} HTTP API{version_tag}");

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {name}\n"));
    // Frontmatter values must stay single-line for the loader's YAML parser.
    out.push_str(&format!("description: {}\n", description.replace(['\r', '\n'], " ")));
    out.push_str("---\n\n");

    out.push_str(&format!("# {display}\n\n"));
    match base_url {
        Some(base) => out.push_str(&format!("Base URL: `{base}`\n\n")),
        None => out.push_str(
            "No base URL was recorded at import time — set `BASE_URL` in the shell before\n\
             running the examples below.\n\n",
        ),
    }
    out.push_str(
        "Call these endpoints with `curl` (or any other shell HTTP tool). Path templates\n\
         like `{id}` are path parameters — substitute a real value. Check the API's own\n\
         authentication requirements before calling.\n\n",
    );

    if endpoints.is_empty() {
        out.push_str("The imported document declares no endpoints.\n");
        return out;
    }

    for ep in endpoints {
        out.push_str(&format!("## `{} {}`\n", ep.method.to_ascii_uppercase(), ep.path));
        if let Some(summary) = ep.summary.as_deref().map(str::trim)
            && !summary.is_empty()
        {
            out.push_str(&format!("{summary}\n"));
        }
        if let Some(op) = ep.operation_id.as_deref().map(str::trim)
            && !op.is_empty()
        {
            out.push_str(&format!("(operationId: {op})\n"));
        }
        let target = match base_url {
            Some(base) => format!("\"{base}{}\"", ep.path),
            None => format!("\"$BASE_URL{}\"", ep.path),
        };
        out.push_str(&format!(
            "\n```bash\ncurl -X {} {}\n```\n\n",
            ep.method.to_ascii_uppercase(),
            target
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::run_one_devops_migrations;
    use serde_json::json;

    async fn service() -> DevopsService {
        // Single connection so the in-memory database outlives one call.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_one_devops_migrations(&dream_core_db::DbPool::Sqlite(pool.clone()))
            .await
            .unwrap();
        DevopsService::new(dream_core_db::DbPool::Sqlite(pool.clone()))
    }

    /// A minimal OpenAPI 3 document: 2 paths x 2 methods.
    fn sample_spec() -> Value {
        json!({
            "openapi": "3.0.1",
            "info": { "title": "Pet Store", "version": "1.0.0" },
            "servers": [{ "url": "https://petstore.example/api" }],
            "paths": {
                "/pets": {
                    "get": { "summary": "List pets", "operationId": "listPets" },
                    "post": { "summary": "Create a pet", "operationId": "createPet" }
                },
                "/pets/{petId}": {
                    "get": { "summary": "Fetch one pet", "operationId": "showPetById" }
                }
            }
        })
    }

    fn manual_input(name: &str, code: &str) -> ApiAssetProfileInput {
        ApiAssetProfileInput {
            name: Some(name.into()),
            code: Some(code.into()),
            base_url: Some("https://api.example.com".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn manual_create_stores_the_operator_authored_profile() {
        let svc = service().await;
        let asset = svc
            .create_api_asset(
                "t1",
                "admin1",
                &ApiAssetProfileInput {
                    category: Some("media".into()),
                    tool_prefix: Some("rec".into()),
                    description: Some("recording backend".into()),
                    ..manual_input("Recording Service", "recording_service")
                },
            )
            .await
            .unwrap();

        assert_eq!(asset.source_format, "manual");
        assert_eq!(asset.code.as_deref(), Some("recording_service"));
        assert_eq!(asset.category.as_deref(), Some("media"));
        assert_eq!(asset.tool_prefix.as_deref(), Some("rec"));
        assert_eq!(asset.auth_type, "none");
        assert!(asset.enabled);
        assert_eq!(asset.endpoint_count, 0);
    }

    #[tokio::test]
    async fn codes_are_normalised_and_unique_per_tenant() {
        let svc = service().await;
        svc.create_api_asset("t1", "admin1", &manual_input("A", "Billing_API"))
            .await
            .unwrap();

        // Uppercase input is folded, so the "different" code collides.
        let clash = svc
            .create_api_asset("t1", "admin1", &manual_input("B", "billing_api"))
            .await;
        assert!(matches!(clash, Err(DevopsError::BadRequest(_))), "{clash:?}");

        // Another tenant may reuse it.
        svc.create_api_asset("t2", "admin1", &manual_input("C", "billing_api"))
            .await
            .unwrap();

        // Punctuation that would break a tool name is refused outright.
        let bad = svc
            .create_api_asset("t1", "admin1", &manual_input("D", "bad code.v2"))
            .await;
        assert!(matches!(bad, Err(DevopsError::BadRequest(_))), "{bad:?}");
    }

    #[tokio::test]
    async fn auth_config_is_write_only_and_survives_an_edit_that_omits_it() {
        let svc = service().await;
        let created = svc
            .create_api_asset(
                "t1",
                "admin1",
                &ApiAssetProfileInput {
                    auth_type: Some("bearer".into()),
                    auth_config: Some(r#"{"token":"sk_live_secret"}"#.into()),
                    ..manual_input("Secure", "secure_api")
                },
            )
            .await
            .unwrap();
        assert!(created.auth_configured);

        // The credential must not be reachable through any read payload.
        let detail = svc.get_api_asset("t1", &created.id).await.unwrap();
        let rendered = serde_json::to_string(&detail).unwrap();
        assert!(!rendered.contains("sk_live_secret"), "credential leaked: {rendered}");

        // An edit that does not mention authConfig keeps it — this is the
        // form-save path, where the field is deliberately never populated.
        let renamed = svc
            .update_api_asset(
                "t1",
                &created.id,
                &ApiAssetProfileInput {
                    name: Some("Secure v2".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(renamed.name, "Secure v2");
        assert!(renamed.auth_configured, "omitting authConfig must not clear it");
        assert_eq!(renamed.auth_type, "bearer");

        // An explicit empty string is how the operator clears it.
        let cleared = svc
            .update_api_asset(
                "t1",
                &created.id,
                &ApiAssetProfileInput {
                    auth_config: Some("   ".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(!cleared.auth_configured);
    }

    #[tokio::test]
    async fn a_blank_optional_field_clears_to_null_never_to_an_empty_string() {
        let svc = service().await;
        let created = svc
            .create_api_asset(
                "t1",
                "admin1",
                &ApiAssetProfileInput {
                    category: Some("media".into()),
                    tool_prefix: Some("rec".into()),
                    ..manual_input("Recording", "recording_service")
                },
            )
            .await
            .unwrap();
        assert_eq!(created.tool_prefix.as_deref(), Some("rec"));

        // The console posts every optional input, so an untouched box arrives
        // as "". That must read as "unset", not as an empty prefix — callers
        // treat None as "fall back to the code".
        let saved = svc
            .update_api_asset(
                "t1",
                &created.id,
                &ApiAssetProfileInput {
                    name: Some("Recording".into()),
                    code: Some("recording_service".into()),
                    tool_prefix: Some("   ".into()),
                    category: Some("".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(saved.tool_prefix, None, "blank prefix must be NULL, not an empty string");
        assert_eq!(saved.category, None);

        // An omitted field is still left alone.
        let untouched = svc
            .update_api_asset(
                "t1",
                &created.id,
                &ApiAssetProfileInput {
                    description: Some("now described".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(untouched.base_url.as_deref(), Some("https://api.example.com"));
        assert_eq!(untouched.description.as_deref(), Some("now described"));
    }

    #[tokio::test]
    async fn auth_input_is_validated() {
        let svc = service().await;
        let bad_type = svc
            .create_api_asset(
                "t1",
                "admin1",
                &ApiAssetProfileInput {
                    auth_type: Some("kerberos".into()),
                    ..manual_input("X", "x_api")
                },
            )
            .await;
        assert!(matches!(bad_type, Err(DevopsError::BadRequest(_))), "{bad_type:?}");

        let bad_json = svc
            .create_api_asset(
                "t1",
                "admin1",
                &ApiAssetProfileInput {
                    auth_type: Some("api_key".into()),
                    auth_config: Some("not json".into()),
                    ..manual_input("Y", "y_api")
                },
            )
            .await;
        assert!(matches!(bad_json, Err(DevopsError::BadRequest(_))), "{bad_json:?}");
    }

    #[tokio::test]
    async fn imported_assets_keep_working_after_017() {
        let svc = service().await;
        let asset = svc
            .import_api_asset("t1", "admin1", "petstore", &sample_spec())
            .await
            .unwrap();
        // The document path sets none of the 017 columns, and must not be
        // forced to: the row is valid with a NULL code and default auth.
        assert_eq!(asset.code, None);
        assert_eq!(asset.auth_type, "none");
        assert!(!asset.auth_configured);
        assert!(asset.enabled);

        // Two imports with no code must not collide on the unique index.
        svc.import_api_asset("t1", "admin1", "petstore-2", &sample_spec())
            .await
            .unwrap();
        assert_eq!(svc.list_api_assets("t1").await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn import_parses_endpoints_and_metadata() {
        let svc = service().await;
        let asset = svc
            .import_api_asset("t1", "admin1", "petstore", &sample_spec())
            .await
            .unwrap();

        assert_eq!(asset.source_format, "openapi");
        assert_eq!(asset.title.as_deref(), Some("Pet Store"));
        assert_eq!(asset.version.as_deref(), Some("1.0.0"));
        assert_eq!(asset.base_url.as_deref(), Some("https://petstore.example/api"));
        let endpoints: Vec<ApiEndpoint> = serde_json::from_value(asset.endpoints).unwrap();
        assert_eq!(endpoints.len(), 3);
        assert!(endpoints.contains(&ApiEndpoint {
            method: "get".into(),
            path: "/pets".into(),
            summary: Some("List pets".into()),
            operation_id: Some("listPets".into()),
        }));
        assert!(
            endpoints
                .iter()
                .all(|e| e.path != "/pets/{petId}".to_owned() || e.method == "get")
        );

        // The raw spec round-trips verbatim through the detail endpoint.
        let detail = svc.get_api_asset("t1", &asset.id).await.unwrap();
        assert_eq!(detail.spec, sample_spec());
    }

    #[tokio::test]
    async fn import_accepts_swagger2_with_host_base_url() {
        let svc = service().await;
        let spec = json!({
            "swagger": "2.0",
            "info": { "title": "Legacy", "version": "2" },
            "host": "legacy.example",
            "basePath": "/v2",
            "paths": { "/a": { "get": {} } }
        });
        let asset = svc.import_api_asset("t1", "admin1", "legacy", &spec).await.unwrap();
        assert_eq!(asset.source_format, "swagger");
        assert_eq!(asset.base_url.as_deref(), Some("https://legacy.example/v2"));
        assert_eq!(asset.endpoint_count, 1);
    }

    #[tokio::test]
    async fn import_rejects_invalid_specs() {
        let svc = service().await;

        // Not an object at all.
        assert!(matches!(
            svc.import_api_asset("t1", "admin1", "x", &json!("not an object")).await,
            Err(DevopsError::BadRequest(_))
        ));
        // A JSON object but no version field.
        assert!(matches!(
            svc.import_api_asset("t1", "admin1", "x", &json!({ "paths": {} })).await,
            Err(DevopsError::BadRequest(_))
        ));
        // Version present but no paths.
        assert!(matches!(
            svc.import_api_asset("t1", "admin1", "x", &json!({ "openapi": "3.0.0", "info": {} }))
                .await,
            Err(DevopsError::BadRequest(_))
        ));
        // paths not an object.
        assert!(matches!(
            svc.import_api_asset("t1", "admin1", "x", &json!({ "openapi": "3.0.0", "paths": 7 }))
                .await,
            Err(DevopsError::BadRequest(_))
        ));
        // Empty name.
        assert!(matches!(
            svc.import_api_asset("t1", "admin1", "  ", &sample_spec()).await,
            Err(DevopsError::BadRequest(_))
        ));
    }

    /// The ordering above, pinned so it cannot regress on a slow machine.
    ///
    /// `list_orders_and_soft_delete_hides` creates two assets back to back and
    /// asserts newest-first, but whether they land in the same millisecond is
    /// up to the host: locally they usually do not and it passes, on a loaded
    /// CI runner they did and it failed with the older asset at index 0. This
    /// forces the tie instead of hoping for it, so it fails every time the
    /// tiebreak is missing rather than once in a while.
    #[tokio::test]
    async fn list_orders_newest_first_when_created_at_ties() {
        let svc = service().await;
        let a = svc
            .import_api_asset("t1", "admin1", "first", &sample_spec())
            .await
            .unwrap();
        let b = svc
            .import_api_asset("t1", "admin1", "second", &sample_spec())
            .await
            .unwrap();

        // Collapse both rows onto one timestamp: the state a same-millisecond
        // burst produces, without depending on the host being fast enough.
        svc.db
            .execute("UPDATE one_api_assets SET created_at = 1700000000000", &db_params![])
            .await
            .unwrap();

        let listed = svc.list_api_assets("t1").await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed[0].id, b.id,
            "with created_at tied, the later import must still come first"
        );
        assert_eq!(listed[1].id, a.id);
    }

    #[tokio::test]
    async fn list_orders_and_soft_delete_hides() {
        let svc = service().await;
        let a = svc
            .import_api_asset("t1", "admin1", "first", &sample_spec())
            .await
            .unwrap();
        let b = svc
            .import_api_asset(
                "t1",
                "admin1",
                "second",
                &json!({
                    "openapi": "3.0.0", "paths": { "/x": { "get": {} } }
                }),
            )
            .await
            .unwrap();

        let listed = svc.list_api_assets("t1").await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, b.id, "newest first");
        assert_eq!(listed[1].id, a.id);
        assert_eq!(listed[1].endpoint_count, 3);

        svc.delete_api_asset("t1", &a.id).await.unwrap();
        let listed = svc.list_api_assets("t1").await.unwrap();
        assert_eq!(listed.len(), 1);
        // Soft-deleted rows are gone from detail lookups too, and deleting
        // again is a 404.
        assert!(matches!(
            svc.get_api_asset("t1", &a.id).await,
            Err(DevopsError::NotFound(_))
        ));
        assert!(matches!(
            svc.delete_api_asset("t1", &a.id).await,
            Err(DevopsError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn tenants_are_isolated() {
        let svc = service().await;
        let a = svc
            .import_api_asset("t1", "admin1", "mine", &sample_spec())
            .await
            .unwrap();

        assert!(svc.list_api_assets("t2").await.unwrap().is_empty());
        assert!(matches!(
            svc.get_api_asset("t2", &a.id).await,
            Err(DevopsError::NotFound(_))
        ));
        assert!(matches!(
            svc.delete_api_asset("t2", &a.id).await,
            Err(DevopsError::NotFound(_))
        ));
        // Another tenant importing the same name does not see or touch t1's row.
        let b = svc
            .import_api_asset(
                "t2",
                "admin2",
                "mine",
                &json!({
                    "openapi": "3.0.0", "paths": { "/y": { "get": {} } }
                }),
            )
            .await
            .unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(svc.list_api_assets("t1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn publish_creates_skill_and_republish_updates_it() {
        let svc = service().await;
        let asset = svc
            .import_api_asset("t1", "admin1", "petstore", &sample_spec())
            .await
            .unwrap();

        let skill = svc
            .publish_api_asset_skill("t1", "admin1", &asset.id, None, false)
            .await
            .unwrap();
        assert_eq!(skill.name, "petstore");
        assert_eq!(skill.scope, "org");
        assert_eq!(skill.visibility, "all");
        assert!(skill.published, "registry rows default to published");
        // The generated SKILL.md carries frontmatter and a curl example per
        // endpoint — the exact bytes team-sync materializes onto members.
        assert!(skill.content.starts_with("---\nname: petstore\n"));
        assert!(
            skill.content.contains("## `GET /pets`"),
            "content was:\n{}",
            skill.content
        );
        assert!(
            skill
                .content
                .contains("curl -X GET \"https://petstore.example/api/pets\"")
        );
        assert!(skill.content.contains("## `GET /pets/{petId}`"));
        assert!(!skill.content.contains("POST") || skill.content.contains("## `POST /pets`"));

        // The asset now links to the skill.
        let after = svc.get_api_asset("t1", &asset.id).await.unwrap();
        assert_eq!(after.asset.published_skill_id.as_deref(), Some(skill.id.as_str()));

        // Re-publish UPDATES the same registry entry (no second row), even
        // after the asset's name would collide with the existing skill.
        let again = svc
            .publish_api_asset_skill("t1", "admin1", &asset.id, Some("https://staging.example"), false)
            .await
            .unwrap();
        assert_eq!(again.id, skill.id);
        assert!(again.content.contains("https://staging.example"));
        assert!(
            !again.content.contains("petstore.example"),
            "override replaces the base url"
        );
        let after = svc.get_api_asset("t1", &asset.id).await.unwrap();
        assert_eq!(after.asset.published_skill_id.as_deref(), Some(skill.id.as_str()));
    }

    #[tokio::test]
    async fn publish_name_collision_is_rejected_not_hijacked() {
        let svc = service().await;
        // An unrelated pre-existing skill owns the name.
        svc.upsert_skill(
            None,
            "petstore",
            "human-made",
            "do things",
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

        let asset = svc
            .import_api_asset("t1", "admin1", "petstore", &sample_spec())
            .await
            .unwrap();
        assert!(matches!(
            svc.publish_api_asset_skill("t1", "admin1", &asset.id, None, false)
                .await,
            Err(DevopsError::BadRequest(_))
        ));
        // The unrelated skill is untouched.
        let skills = svc.list_skills("admin1").await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].content, "do things");
    }

    #[tokio::test]
    async fn publish_requires_the_asset_in_the_callers_tenant() {
        let svc = service().await;
        let asset = svc
            .import_api_asset("t1", "admin1", "petstore", &sample_spec())
            .await
            .unwrap();
        assert!(matches!(
            svc.publish_api_asset_skill("t2", "admin2", &asset.id, None, false)
                .await,
            Err(DevopsError::NotFound(_))
        ));
    }

    #[test]
    fn skill_md_without_base_url_uses_env_var() {
        let md = build_api_asset_skill_md(
            "naked",
            None,
            None,
            None,
            &[ApiEndpoint {
                method: "post".into(),
                path: "/items".into(),
                summary: None,
                operation_id: None,
            }],
        );
        assert!(md.contains("BASE_URL"));
        assert!(md.contains("curl -X POST \"$BASE_URL/items\""));
        assert!(
            md.starts_with("---\nname: naked\ndescription: Call the naked HTTP API\n---\n"),
            "md was: {md}"
        );
    }
}
