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

/// One security policy template (P1-8 安全策略模板层, second layer of the
/// reference product's 全局基线/策略模板 model): a *named snapshot* of the
/// same seven policy fields the tenant baseline carries. Storing or binding
/// one changes nothing by itself — only an explicit `apply` copies its fields
/// into the tenant baseline that enforcement actually reads. `binding_count`
/// is the "覆盖实例数" the template list shows: how many subjects the
/// template has been allocated to, not how many are governed by it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityPolicyTemplateDto {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Which built-in tier the fields were authored from — provenance only.
    pub tier: String,
    pub terminal_tools_require_approval: bool,
    pub destructive_commands_blocked: bool,
    pub blocked_command_patterns: Vec<String>,
    pub external_network_denied_by_default: bool,
    pub message_scan_enabled: bool,
    pub message_redact_enabled: bool,
    pub send_rate_limit_per_minute: Option<i64>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub binding_count: i64,
}

/// One template's assignment to a subject (a member or a whole department).
/// An allocation record, not a live grant — see the migration 009 header for
/// the three-layer semantics and why enforcement stays on the baseline.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyTemplateBindingDto {
    pub id: String,
    pub template_id: String,
    /// `"member" | "department"`.
    pub subject_type: String,
    pub subject_id: String,
    pub note: Option<String>,
    pub bound_by: String,
    pub bound_at: i64,
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

/// One member's IM bot channels, for the enterprise oversight view of a
/// personal-edition feature (`dream-core-channel`). Read-only: an admin sees
/// who connected a bot on which platform and how many external IM users it
/// authorized. The bots themselves stay owner-managed in dream-ui.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImChannelMemberDto {
    pub user_id: String,
    pub display_name: String,
    pub plugins: Vec<ImChannelPluginDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImChannelPluginDto {
    /// `telegram` | `lark` | `dingtalk` | `slack` | `discord` | `weixin` | …
    pub platform: String,
    pub name: String,
    pub enabled: bool,
    /// The plugin's last connection status string, or `None` if never connected.
    pub status: Option<String>,
    pub last_connected: Option<i64>,
    /// Distinct external IM users this member's bot has authorized.
    pub authorized_user_count: i64,
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

/// One member's file vault as the admin governance page shows it ("个人文件
/// 仓库", align-openocta P2-4): availability status, optional quota, and
/// usage aggregated from the object ledger. A member with no settings row
/// reads as available / unlimited — see the migration's comment.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVaultDto {
    pub user_id: String,
    /// `"available" | "frozen"`.
    pub status: String,
    pub quota_bytes: Option<i64>,
    pub usage_bytes: i64,
    pub object_count: i64,
}

/// One stored vault object, admin or owner view. `deleted_at` is set for
/// tombstoned rows (the disk file is gone; the ledger row stays for the
/// audit trail).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVaultObjectDto {
    pub id: String,
    pub file_name: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

/// One member's reconciliation result: what the ledger claims vs what the
/// storage directory actually holds.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVaultReconcileEntry {
    pub user_id: String,
    /// Ledger rows with no file on disk (data loss or out-of-band deletion).
    pub missing_on_disk: Vec<String>,
    /// Files on disk with no ledger row (out-of-band writes).
    pub missing_in_ledger: Vec<String>,
    /// Ledger rows whose on-disk size differs from `size_bytes`.
    pub size_mismatches: Vec<String>,
}

/// What a DTO carries instead of a sensitive value. The backend substitutes
/// this placeholder into the redacted `value` field, so a frontend exporting
/// the list to CSV needs no special-casing — sensitive cells are already
/// unreadable placeholders.
pub const SENSITIVE_PLACEHOLDER: &str = "<sensitive>";

/// One named configuration set (P1-5 config vault) — the alias consumers
/// write into `{{config.<name>.<key>}}`. `entry_count` is an aggregate for
/// the admin list; `ref_count` is the runtime-computed number of skill
/// bodies embedding a reference to this set (see
/// `PlatformService::config_set_references` for how it is derived and its
/// boundaries) — never stored.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSetDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub entry_count: i64,
    pub ref_count: i64,
}

/// One key/value entry of a config set, as every read surface returns it.
/// A sensitive entry's `value` is [`SENSITIVE_PLACEHOLDER`] — the plaintext
/// is encrypted at rest and never crosses a DTO; `has_value` exists so the
/// UI can distinguish "a value is stored but hidden" from "no value yet"
/// even though entries are currently always non-empty.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEntryDto {
    pub id: String,
    pub set_id: String,
    pub key: String,
    pub value: String,
    pub sensitive: bool,
    pub has_value: bool,
}

/// One consumer found embedding `{{config.<set-alias>.<key>}}` in its body.
/// Today the only scanned surface is devops' `one_skill_registry.content`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSetReference {
    /// The referencing skill's registry id (usable as a devops deep link).
    pub skill_id: String,
    pub skill_name: String,
}

/// The reference report for one config set: who consumes it and how many
/// references exist — the number an admin checks before deleting a set.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSetReferencesDto {
    pub set_id: String,
    pub set_name: String,
    pub count: i64,
    pub references: Vec<ConfigSetReference>,
}

/// Result of a bulk import (P1-5 "Excel 批量迁移"): `imported` is the number
/// of entries actually upserted (distinct keys), `skipped` the number of
/// input rows dropped — in-batch duplicates superseded by a later row with
/// the same key, and rows with an empty key.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigBulkImportDto {
    pub imported: i64,
    pub skipped: i64,
}
