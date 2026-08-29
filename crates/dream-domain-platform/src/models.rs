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

/// Security policy baseline (E5). One per tenant — see the migration's own
/// doc comment for why this is a single baseline and not a template library.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityPolicyDto {
    /// `"relaxed" | "standard" | "strict" | "custom"`.
    pub tier: String,
    pub terminal_tools_require_approval: bool,
    pub destructive_commands_blocked: bool,
    pub blocked_command_patterns: Vec<String>,
    pub external_network_denied_by_default: bool,
    pub message_scan_enabled: bool,
    pub message_redact_enabled: bool,
    pub send_rate_limit_per_minute: Option<i64>,
    pub updated_at: Option<i64>,
}

/// An open-integration API key (E5), redacted — never carries the secret or
/// its hash. `key_prefix` is enough to tell keys apart in a list without
/// ever re-displaying the full secret.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyDto {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
    pub allowed_paths: Vec<String>,
    pub rate_limit_per_minute: Option<i64>,
    /// `"active" | "revoked"`.
    pub status: String,
    pub created_by: String,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

/// What `PlatformService::create_api_key` returns: the redacted record plus
/// the plaintext secret, which exists in this shape exactly once — the
/// caller must show it to the admin now, because it is never recoverable
/// again (only `ApiKeyDto.key_prefix` and a hash survive past this call).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewApiKeyDto {
    #[serde(flatten)]
    pub key: ApiKeyDto,
    pub secret: String,
}

/// A scene (E5 "场景管理"): a named bundle of resource grants + descriptive
/// job-function tags. `member_count` is the current roster size, shown in
/// the admin UI's scene list without a second round trip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneDto {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub job_functions: Vec<String>,
    pub built_in: bool,
    pub member_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One sent in-app notification, as the admin's sent-history list shows it
/// ("站内消息", align-openocta P2-3). `recipient_count` / `read_count` are
/// aggregates, not rows: a broadcast has an unbounded audience (every
/// current and future member of the tenant) so its `recipient_count` is
/// the tenant's current roster size at read time.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDto {
    pub id: String,
    /// `"broadcast" | "targeted"`.
    pub kind: String,
    pub category: String,
    pub title: String,
    pub body: String,
    pub recipient_count: i64,
    pub read_count: i64,
    pub created_by: String,
    pub created_at: i64,
}

/// One in-app notification as its recipient sees it — the member-facing
/// inbox / the home page's unread card. `read_at` is `None` while unread.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyNotificationDto {
    pub id: String,
    pub kind: String,
    pub category: String,
    pub title: String,
    pub body: String,
    pub created_by: String,
    pub created_at: i64,
    pub read_at: Option<i64>,
}

/// The member-facing inbox page plus the count the home page's unread
/// aggregation wants, in one round trip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyNotificationsDto {
    pub notifications: Vec<MyNotificationDto>,
    pub unread_count: i64,
}
