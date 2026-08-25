//! Redacted DTOs for the platform-config admin UI. Secrets are never echoed
//! back — only a `has*` presence flag, same as `dream_domain_org::SmtpConfigDto`.

use serde::Serialize;

/// Redacted container-runtime config (P1-3). A real run additionally requires a
/// `ContainerRuntime` implementation wired at the app layer; until then a probe
/// reports "not configured".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerConfigDto {
    pub runtime_kind: Option<String>,
    pub endpoint: Option<String>,
    pub default_image: Option<String>,
    pub registry: Option<String>,
    pub has_registry_secret: bool,
    pub enabled: bool,
    pub updated_at: Option<i64>,
}

/// Redacted realtime-collaboration config (P2-2). Same reserved-adapter shape:
/// a real presence/sync backend requires a `CollaborationProvider` wired at the
/// app layer; until then a probe reports "not configured".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationConfigDto {
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    pub has_secret: bool,
    pub presence: bool,
    pub enabled: bool,
    pub updated_at: Option<i64>,
}

/// IP allowlist config (P1-4). `cidrs` is the parsed list of allowed CIDR/IP
/// strings. Enforcement (request blocking) is a reserved drop-in — storing this
/// does not by itself block anyone.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpAllowlistConfigDto {
    pub cidrs: Vec<String>,
    pub enabled: bool,
    pub updated_at: Option<i64>,
}

/// SIEM audit-log export config (P1-4), redacted. A real export requires a
/// `SiemExporter` wired at the app layer; until then a probe reports
/// "not configured". The token is never echoed — only `has_secret`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiemConfigDto {
    pub kind: Option<String>,
    pub endpoint: Option<String>,
    pub has_secret: bool,
    pub enabled: bool,
    pub updated_at: Option<i64>,
}

/// One row of the resource-authorization matrix (E5): `subject` (a member or
/// a whole department) may reach `resource` (one skill / MCP server / digital
/// employee / model channel, or every one of that type via `resource_id ==
/// "*"`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceGrantDto {
    pub id: String,
    pub subject_type: String,
    pub subject_id: String,
    pub resource_type: String,
    pub resource_id: String,
    pub granted_by: String,
    pub created_at: i64,
}

/// A member's resolved access to one resource type, after expanding both
/// their own direct grants and every department grant reached by walking
/// their department's ancestor chain. `all: true` short-circuits
/// `resource_ids` (a wildcard grant makes the explicit id list moot); the
/// list is otherwise the set of specific resource ids the caller may reach.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveGrantDto {
    pub all: bool,
    pub resource_ids: Vec<String>,
}
