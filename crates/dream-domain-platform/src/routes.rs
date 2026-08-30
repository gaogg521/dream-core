//! `/api/one/admin/platform/*` routes — deployment infrastructure config
//! (P1-3 container runtime + P2-2 realtime collaboration), the E5
//! resource-authorization matrix (`resource-grants*`), and E5 scene
//! management (`scenes*`) — a named bundle of resource grants a member gets
//! in one action by joining the scene, instead of an admin granting each
//! skill/tool/model/channel one at a time — the E5 security policy baseline
//! (`security-policy*`, three built-in tiers or a field-by-field custom
//! override) — and E5 open-integration API keys (`api-keys*`). All three
//! "E5" pieces share the same posture: see each migration's own doc comment
//! for exactly what is and isn't enforced yet.
//!
//! Mounted behind the upstream `auth_middleware` (relies on `CurrentUser` in
//! request extensions). All routes are gated by `RequirePlatformAdmin`.
//!
//! ⚠️ The resource-grants endpoints only cover four resource types, so check
//! before assuming a change here is inert:
//!
//! - **Enforced** (`dream-domain-devops` widens its read predicate with
//!   `effective_resource_ids` via the `ResourceGrantSource` seam): `skill`,
//!   `mcp`, `knowledge`, `model_channel`. Changing grant semantics changes
//!   what members actually see on those four read paths.
//! - **Rejected outright**: `employee` — digital employees are an
//!   owner-centric asset (private/shared, not a per-subject grant registry),
//!   so `grant_resource` returns 400 instead of recording a grant that
//!   nothing would ever consult. See `PlatformService::GRANT_RESOURCE_TYPES`.
//!
//! A grant only ever *adds* reachability on the enforced paths, and never
//! widens tenancy (see `DevopsService::widen_with_grants`).

use axum::extract::{Multipart, Path, Query, State};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use dream_core_api_types::ApiResponse;
use dream_core_auth::CurrentUser;

use crate::collaboration::CollaborationStatus;
use crate::container::ContainerStatus;
use crate::error::PlatformError;
use crate::models::{
    ApiKeyDto, CollaborationConfigDto, ConfigBulkImportDto, ConfigEntryDto, ConfigSetDto, ConfigSetReferencesDto,
    ContainerConfigDto, EffectiveGrantDto, FileVaultDto, FileVaultObjectDto, FileVaultReconcileEntry,
    IpAllowlistConfigDto, MyNotificationsDto, NewApiKeyDto, NotificationDto, PolicyTemplateBindingDto,
    ResourceGrantDto, SceneDto, SecurityPolicyDto, SecurityPolicyTemplateDto, SiemConfigDto,
};
use crate::rbac::{RequirePlatformAdmin, RequirePlatformMember};
use crate::service::ConfigImportRow;
use crate::siem::SiemStatus;
use crate::state::OnePlatformRouterState;

pub fn one_platform_routes(state: OnePlatformRouterState) -> Router {
    Router::new()
        .route(
            "/api/one/admin/platform/container",
            get(get_container).put(set_container),
        )
        .route("/api/one/admin/platform/container/probe", post(probe_container))
        .route(
            "/api/one/admin/platform/collaboration",
            get(get_collaboration).put(set_collaboration),
        )
        .route("/api/one/admin/platform/collaboration/probe", post(probe_collaboration))
        .route(
            "/api/one/admin/platform/ip-allowlist",
            get(get_ip_allowlist).put(set_ip_allowlist),
        )
        .route("/api/one/admin/platform/ip-allowlist/check", post(check_ip_allowlist))
        .route("/api/one/admin/platform/siem", get(get_siem).put(set_siem))
        .route("/api/one/admin/platform/siem/probe", post(probe_siem))
        .route(
            "/api/one/admin/platform/resource-grants",
            get(list_resource_grants).post(create_resource_grant),
        )
        .route(
            "/api/one/admin/platform/resource-grants/{id}",
            delete(delete_resource_grant),
        )
        .route(
            "/api/one/admin/platform/resource-grants/effective",
            get(effective_resource_grants),
        )
        .route("/api/one/admin/platform/scenes", get(list_scenes).post(create_scene))
        .route(
            "/api/one/admin/platform/scenes/{id}",
            axum::routing::put(update_scene).delete(delete_scene),
        )
        .route(
            "/api/one/admin/platform/scenes/{id}/members",
            get(list_scene_members).post(add_scene_member),
        )
        .route(
            "/api/one/admin/platform/scenes/{id}/members/{user_id}",
            delete(remove_scene_member),
        )
        .route(
            "/api/one/admin/platform/security-policy",
            get(get_security_policy).put(set_security_policy),
        )
        .route(
            "/api/one/admin/platform/security-policy/tier",
            post(apply_security_policy_tier),
        )
        // P1-8 安全策略模板层: named policy snapshots, independently bound to
        // members/departments, copied into the tenant baseline by an explicit
        // apply. Binding never changes enforcement by itself — see the
        // migration 009 header for the three-layer semantics.
        .route(
            "/api/one/admin/platform/security-policy/templates",
            get(list_policy_templates).post(create_policy_template),
        )
        .route(
            "/api/one/admin/platform/security-policy/templates/{id}",
            axum::routing::put(update_policy_template).delete(delete_policy_template),
        )
        .route(
            "/api/one/admin/platform/security-policy/templates/{id}/apply",
            post(apply_policy_template),
        )
        .route(
            "/api/one/admin/platform/security-policy/templates/{id}/bindings",
            get(list_policy_template_bindings).post(bind_policy_template),
        )
        .route(
            "/api/one/admin/platform/security-policy/bindings/{bindingId}",
            axum::routing::delete(unbind_policy_template),
        )
        .route(
            "/api/one/admin/platform/api-keys",
            get(list_api_keys).post(create_api_key),
        )
        .route("/api/one/admin/platform/api-keys/{id}", delete(revoke_api_key))
        .route(
            "/api/one/admin/platform/notifications",
            get(list_notifications).post(create_notification),
        )
        .route(
            "/api/one/admin/platform/notifications/{id}",
            axum::routing::delete(delete_notification),
        )
        // Member-facing self-service half of the in-app notifications — any
        // enterprise member (any role) reads their own inbox and marks
        // messages read; composing stays admin-only above.
        .route("/api/one/notifications", get(my_notifications))
        .route("/api/one/notifications/read", post(mark_notifications_read))
        // P2-4 personal file vault — member self-service half. Uploads are
        // 10 MiB-capped by the shared `BODY_LIMIT`; frozen vaults refuse
        // uploads but keep existing objects readable/deletable.
        .route("/api/one/vault", get(my_vault))
        .route("/api/one/vault/files", get(list_my_vault_files).post(upload_vault_file))
        .route(
            "/api/one/vault/files/{id}",
            get(download_vault_file).delete(delete_vault_file),
        )
        // P2-4 admin governance half: per-member status/quota/usage plus the
        // ledger-vs-disk reconciliation pass.
        .route("/api/one/admin/platform/file-vault", get(admin_list_file_vaults))
        .route(
            "/api/one/admin/platform/file-vault/reconcile",
            post(admin_reconcile_file_vaults),
        )
        .route(
            "/api/one/admin/platform/file-vault/{user_id}/status",
            axum::routing::put(admin_set_file_vault_status),
        )
        .route(
            "/api/one/admin/platform/file-vault/{user_id}/quota",
            axum::routing::put(admin_set_file_vault_quota),
        )
        .route(
            "/api/one/admin/platform/file-vault/{user_id}/objects",
            get(admin_list_file_vault_objects),
        )
        // P1-5 config vault (配置项) — named configuration sets + key/value
        // entries that skills/tools reference via `{{config.<alias>.<key>}}`.
        // Sensitive entries are encrypted at rest and every read here returns
        // a "<sensitive>" placeholder, never the plaintext. All admin-only.
        .route(
            "/api/one/admin/platform/config-sets",
            get(list_config_sets).post(create_config_set),
        )
        .route(
            "/api/one/admin/platform/config-sets/{id}",
            axum::routing::put(update_config_set).delete(delete_config_set),
        )
        .route(
            "/api/one/admin/platform/config-sets/{id}/entries",
            get(list_config_entries).post(put_config_entry),
        )
        .route(
            "/api/one/admin/platform/config-entries/{entryId}",
            delete(delete_config_entry),
        )
        .route(
            "/api/one/admin/platform/config-sets/{id}/bulk-import",
            post(bulk_import_config_entries),
        )
        .route(
            "/api/one/admin/platform/config-sets/{id}/references",
            get(config_set_references),
        )
        .with_state(state)
}

// --- P1-3 container runtime ---

async fn get_container(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<ContainerConfigDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.get_container_config(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetContainerBody {
    #[serde(default)]
    runtime_kind: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    default_image: Option<String>,
    #[serde(default)]
    registry: Option<String>,
    /// Absent/empty = keep the stored registry secret.
    #[serde(default)]
    registry_secret: Option<String>,
    #[serde(default)]
    enabled: bool,
}

async fn set_container(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Json(body): Json<SetContainerBody>,
) -> Result<Json<ApiResponse<ContainerConfigDto>>, PlatformError> {
    let dto = state
        .service
        .set_container_config(
            &actor.tenant_id,
            body.runtime_kind.as_deref(),
            body.endpoint.as_deref(),
            body.default_image.as_deref(),
            body.registry.as_deref(),
            body.registry_secret.as_deref(),
            body.enabled,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn probe_container(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<ContainerStatus>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.probe_container(&actor.tenant_id).await?,
    )))
}

// --- P2-2 realtime collaboration ---

async fn get_collaboration(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<CollaborationConfigDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.get_collaboration_config(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetCollaborationBody {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    /// Absent/empty = keep the stored secret.
    #[serde(default)]
    secret: Option<String>,
    #[serde(default)]
    presence: bool,
    #[serde(default)]
    enabled: bool,
}

async fn set_collaboration(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Json(body): Json<SetCollaborationBody>,
) -> Result<Json<ApiResponse<CollaborationConfigDto>>, PlatformError> {
    let dto = state
        .service
        .set_collaboration_config(
            &actor.tenant_id,
            body.provider.as_deref(),
            body.endpoint.as_deref(),
            body.secret.as_deref(),
            body.presence,
            body.enabled,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn probe_collaboration(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<CollaborationStatus>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.probe_collaboration(&actor.tenant_id).await?,
    )))
}

// --- P1-4 IP allowlist ---

async fn get_ip_allowlist(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<IpAllowlistConfigDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.get_ip_allowlist(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetIpAllowlistBody {
    #[serde(default)]
    cidrs: Vec<String>,
    #[serde(default)]
    enabled: bool,
}

async fn set_ip_allowlist(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Json(body): Json<SetIpAllowlistBody>,
) -> Result<Json<ApiResponse<IpAllowlistConfigDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state
            .service
            .set_ip_allowlist(&actor.tenant_id, &body.cidrs, body.enabled)
            .await?,
    )))
}

#[derive(Deserialize)]
struct CheckIpBody {
    ip: String,
}

#[derive(serde::Serialize)]
struct CheckIpResult {
    allowed: bool,
}

/// Test whether an IP would be allowed under the current allowlist — lets an
/// admin validate rules (and confirm they won't lock themselves out) before
/// enabling enforcement.
async fn check_ip_allowlist(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Json(body): Json<CheckIpBody>,
) -> Result<Json<ApiResponse<CheckIpResult>>, PlatformError> {
    let allowed = state.service.is_ip_allowed(&actor.tenant_id, &body.ip).await?;
    Ok(Json(ApiResponse::ok(CheckIpResult { allowed })))
}

// --- P1-4 SIEM export ---

async fn get_siem(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<SiemConfigDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.get_siem_config(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetSiemBody {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    /// Absent/empty = keep the stored token.
    #[serde(default)]
    secret: Option<String>,
    #[serde(default)]
    enabled: bool,
}

async fn set_siem(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Json(body): Json<SetSiemBody>,
) -> Result<Json<ApiResponse<SiemConfigDto>>, PlatformError> {
    let dto = state
        .service
        .set_siem_config(
            &actor.tenant_id,
            body.kind.as_deref(),
            body.endpoint.as_deref(),
            body.secret.as_deref(),
            body.enabled,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn probe_siem(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<SiemStatus>>, PlatformError> {
    Ok(Json(ApiResponse::ok(state.service.probe_siem(&actor.tenant_id).await?)))
}

// --- E5 resource authorization matrix ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListGrantsQuery {
    #[serde(default)]
    subject_type: Option<String>,
    #[serde(default)]
    subject_id: Option<String>,
    #[serde(default)]
    resource_type: Option<String>,
}

async fn list_resource_grants(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Query(query): Query<ListGrantsQuery>,
) -> Result<Json<ApiResponse<Vec<ResourceGrantDto>>>, PlatformError> {
    let grants = state
        .service
        .list_grants(
            &actor.tenant_id,
            query.subject_type.as_deref(),
            query.subject_id.as_deref(),
            query.resource_type.as_deref(),
        )
        .await?;
    Ok(Json(ApiResponse::ok(grants)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateGrantBody {
    subject_type: String,
    subject_id: String,
    resource_type: String,
    resource_id: String,
}

async fn create_resource_grant(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateGrantBody>,
) -> Result<Json<ApiResponse<ResourceGrantDto>>, PlatformError> {
    let dto = state
        .service
        .grant_resource(
            &actor.tenant_id,
            &body.subject_type,
            &body.subject_id,
            &body.resource_type,
            &body.resource_id,
            &user.id,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_resource_grant(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state.service.revoke_resource(&actor.tenant_id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EffectiveGrantsQuery {
    member_id: String,
    resource_type: String,
}

/// What one member can reach for one resource type, resolved through their
/// own grants and their department chain. Admin-gated, same as every other
/// route here — a self-service "what can I see" endpoint for a caller to ask
/// about themselves is a straightforward follow-up.
///
/// ⚠️ This answers "what does the matrix say", which is not the same as "what
/// will the member actually see": `employee` grants are resolved here but
/// enforced nowhere (see the module docs for which types are live).
async fn effective_resource_grants(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Query(query): Query<EffectiveGrantsQuery>,
) -> Result<Json<ApiResponse<EffectiveGrantDto>>, PlatformError> {
    let dto = state
        .service
        .effective_resource_ids(&actor.tenant_id, &query.member_id, &query.resource_type)
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

// --- E5 scene management ---

async fn list_scenes(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<Vec<SceneDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_scenes(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SceneBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    job_functions: Vec<String>,
}

async fn create_scene(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Json(body): Json<SceneBody>,
) -> Result<Json<ApiResponse<SceneDto>>, PlatformError> {
    let dto = state
        .service
        .create_scene(
            &actor.tenant_id,
            &body.name,
            body.description.as_deref(),
            &body.job_functions,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn update_scene(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
    Json(body): Json<SceneBody>,
) -> Result<Json<ApiResponse<SceneDto>>, PlatformError> {
    let dto = state
        .service
        .update_scene(
            &actor.tenant_id,
            &id,
            &body.name,
            body.description.as_deref(),
            &body.job_functions,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_scene(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state.service.delete_scene(&actor.tenant_id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn list_scene_members(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<String>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_scene_members(&actor.tenant_id, &id).await?,
    )))
}

#[derive(Deserialize)]
struct AddSceneMemberBody {
    #[serde(rename = "userId")]
    user_id: String,
}

async fn add_scene_member(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
    Json(body): Json<AddSceneMemberBody>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state
        .service
        .add_scene_member(&actor.tenant_id, &id, &body.user_id)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn remove_scene_member(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path((id, user_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state
        .service
        .remove_scene_member(&actor.tenant_id, &id, &user_id)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

// --- E5 security policy baseline ---

async fn get_security_policy(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<SecurityPolicyDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.get_security_policy(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetSecurityPolicyBody {
    #[serde(default)]
    terminal_tools_require_approval: bool,
    #[serde(default)]
    destructive_commands_blocked: bool,
    #[serde(default)]
    blocked_command_patterns: Vec<String>,
    #[serde(default)]
    external_network_denied_by_default: bool,
    #[serde(default)]
    message_scan_enabled: bool,
    #[serde(default)]
    message_redact_enabled: bool,
    #[serde(default)]
    send_rate_limit_per_minute: Option<i64>,
}

async fn set_security_policy(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Json(body): Json<SetSecurityPolicyBody>,
) -> Result<Json<ApiResponse<SecurityPolicyDto>>, PlatformError> {
    let dto = state
        .service
        .set_security_policy(
            &actor.tenant_id,
            body.terminal_tools_require_approval,
            body.destructive_commands_blocked,
            &body.blocked_command_patterns,
            body.external_network_denied_by_default,
            body.message_scan_enabled,
            body.message_redact_enabled,
            body.send_rate_limit_per_minute,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
struct ApplyTierBody {
    tier: String,
}

async fn apply_security_policy_tier(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Json(body): Json<ApplyTierBody>,
) -> Result<Json<ApiResponse<SecurityPolicyDto>>, PlatformError> {
    let dto = state
        .service
        .apply_security_policy_tier(&actor.tenant_id, &body.tier)
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

// --- P1-8 security policy templates (安全策略模板层) ---

async fn list_policy_templates(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<Vec<SecurityPolicyTemplateDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_policy_templates(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyTemplateBody {
    name: String,
    #[serde(default)]
    description: String,
    /// Which built-in tier the fields were authored from — provenance only.
    #[serde(default = "default_policy_template_tier")]
    tier: String,
    #[serde(default)]
    terminal_tools_require_approval: bool,
    #[serde(default)]
    destructive_commands_blocked: bool,
    #[serde(default)]
    blocked_command_patterns: Vec<String>,
    #[serde(default)]
    external_network_denied_by_default: bool,
    #[serde(default)]
    message_scan_enabled: bool,
    #[serde(default)]
    message_redact_enabled: bool,
    #[serde(default)]
    send_rate_limit_per_minute: Option<i64>,
}

fn default_policy_template_tier() -> String {
    "custom".to_owned()
}

async fn create_policy_template(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<PolicyTemplateBody>,
) -> Result<Json<ApiResponse<SecurityPolicyTemplateDto>>, PlatformError> {
    let dto = state
        .service
        .create_policy_template(
            &actor.tenant_id,
            &body.name,
            &body.description,
            &body.tier,
            body.terminal_tools_require_approval,
            body.destructive_commands_blocked,
            &body.blocked_command_patterns,
            body.external_network_denied_by_default,
            body.message_scan_enabled,
            body.message_redact_enabled,
            body.send_rate_limit_per_minute,
            &user.id,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn update_policy_template(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
    Json(body): Json<PolicyTemplateBody>,
) -> Result<Json<ApiResponse<SecurityPolicyTemplateDto>>, PlatformError> {
    let dto = state
        .service
        .update_policy_template(
            &actor.tenant_id,
            &id,
            &body.name,
            &body.description,
            body.terminal_tools_require_approval,
            body.destructive_commands_blocked,
            &body.blocked_command_patterns,
            body.external_network_denied_by_default,
            body.message_scan_enabled,
            body.message_redact_enabled,
            body.send_rate_limit_per_minute,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_policy_template(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state.service.delete_policy_template(&actor.tenant_id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn apply_policy_template(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<SecurityPolicyDto>>, PlatformError> {
    let dto = state
        .service
        .apply_policy_template(&actor.tenant_id, &id, &user.id)
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn list_policy_template_bindings(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<PolicyTemplateBindingDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state
            .service
            .list_policy_template_bindings(&actor.tenant_id, &id)
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BindPolicyTemplateBody {
    /// `"member" | "department"`.
    subject_type: String,
    subject_id: String,
    #[serde(default)]
    note: Option<String>,
}

async fn bind_policy_template(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<BindPolicyTemplateBody>,
) -> Result<Json<ApiResponse<PolicyTemplateBindingDto>>, PlatformError> {
    let dto = state
        .service
        .bind_policy_template(
            &actor.tenant_id,
            &id,
            &body.subject_type,
            &body.subject_id,
            body.note.as_deref(),
            &user.id,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn unbind_policy_template(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(binding_id): Path<String>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state
        .service
        .unbind_policy_template(&actor.tenant_id, &binding_id)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

// --- E5 open-integration API keys ---

async fn list_api_keys(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<Vec<ApiKeyDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_api_keys(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateApiKeyBody {
    name: String,
    #[serde(default)]
    allowed_paths: Vec<String>,
    #[serde(default)]
    rate_limit_per_minute: Option<i64>,
}

async fn create_api_key(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateApiKeyBody>,
) -> Result<Json<ApiResponse<NewApiKeyDto>>, PlatformError> {
    let dto = state
        .service
        .create_api_key(
            &actor.tenant_id,
            &body.name,
            &body.allowed_paths,
            body.rate_limit_per_minute,
            &user.id,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn revoke_api_key(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state.service.revoke_api_key(&actor.tenant_id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

// --- P2-3 in-app notifications (站内消息) ---

async fn list_notifications(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<Vec<NotificationDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_notifications(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateNotificationBody {
    /// `"broadcast" | "targeted"`.
    kind: String,
    #[serde(default)]
    category: Option<String>,
    title: String,
    body: String,
    /// Only read for `kind = "targeted"`; every id must be a member of the
    /// tenant.
    #[serde(default)]
    recipient_ids: Vec<String>,
}

async fn create_notification(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateNotificationBody>,
) -> Result<Json<ApiResponse<NotificationDto>>, PlatformError> {
    let dto = state
        .service
        .create_notification(
            &actor.tenant_id,
            &body.kind,
            body.category.as_deref().unwrap_or(""),
            &body.title,
            &body.body,
            &body.recipient_ids,
            &user.id,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_notification(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state.service.delete_notification(&actor.tenant_id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn my_notifications(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<MyNotificationsDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state
            .service
            .list_my_notifications(&actor.tenant_id, &user.id, MY_NOTIFICATIONS_LIMIT)
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarkNotificationsReadBody {
    /// Empty = mark every visible unread one ("mark all read").
    #[serde(default)]
    ids: Vec<String>,
}

async fn mark_notifications_read(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<MarkNotificationsReadBody>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state
        .service
        .mark_notifications_read(&actor.tenant_id, &user.id, &body.ids)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

/// The inbox caps the rows it returns (an old tenant can accumulate a long
/// sent history) while `unread_count` always counts the full visible set —
/// see `PlatformService::list_my_notifications`.
const MY_NOTIFICATIONS_LIMIT: i64 = 200;

// --- P2-4 personal file vault — member self-service half ---

async fn my_vault(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<FileVaultDto>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.my_vault(&actor.tenant_id, &user.id).await?,
    )))
}

async fn list_my_vault_files(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<FileVaultObjectDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_my_vault_objects(&actor.tenant_id, &user.id).await?,
    )))
}

/// Multipart upload: exactly one `file` field carrying the bytes and its
/// client-side name. Extra fields are ignored, a missing `file` field is a
/// 400.
async fn upload_vault_file(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<FileVaultObjectDto>>, PlatformError> {
    let mut uploaded: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| PlatformError::BadRequest(format!("invalid multipart body: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let file_name = field.file_name().unwrap_or_default().to_owned();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| PlatformError::BadRequest(format!("failed to read upload: {e}")))?;
        uploaded = Some((file_name, bytes.to_vec()));
        break;
    }
    let Some((file_name, bytes)) = uploaded else {
        return Err(PlatformError::BadRequest(
            "multipart body must carry a 'file' field".into(),
        ));
    };
    let dto = state
        .service
        .upload_vault_object(&actor.tenant_id, &user.id, &file_name, &bytes)
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn download_vault_file(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, PlatformError> {
    let (dto, bytes) = state.service.read_vault_object(&actor.tenant_id, &user.id, &id).await?;
    let disposition = format!(
        "attachment; filename=\"{}\"",
        dto.file_name.replace(['"', '\\', '\r', '\n'], "_")
    );
    Ok(([(axum::http::header::CONTENT_DISPOSITION, disposition)], bytes).into_response())
}

async fn delete_vault_file(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformMember(actor): RequirePlatformMember,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state
        .service
        .delete_vault_object(&actor.tenant_id, &user.id, &id)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

// --- P2-4 personal file vault — admin governance half ---

async fn admin_list_file_vaults(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<Vec<FileVaultDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.admin_list_vaults(&actor.tenant_id).await?,
    )))
}

async fn admin_reconcile_file_vaults(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<Vec<FileVaultReconcileEntry>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.admin_reconcile_vaults(&actor.tenant_id).await?,
    )))
}

async fn admin_set_file_vault_status(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(user_id): Path<String>,
    Json(body): Json<SetVaultStatusBody>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state
        .service
        .admin_set_vault_status(&actor.tenant_id, &user_id, &body.status)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetVaultStatusBody {
    /// `"available" | "frozen"`.
    status: String,
}

async fn admin_set_file_vault_quota(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(user_id): Path<String>,
    Json(body): Json<SetVaultQuotaBody>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state
        .service
        .admin_set_vault_quota(&actor.tenant_id, &user_id, body.quota_bytes)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetVaultQuotaBody {
    /// `null` = unlimited.
    quota_bytes: Option<i64>,
}

async fn admin_list_file_vault_objects(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(user_id): Path<String>,
) -> Result<Json<ApiResponse<Vec<FileVaultObjectDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state
            .service
            .admin_list_vault_objects(&actor.tenant_id, &user_id)
            .await?,
    )))
}

// --- P1-5 config vault (配置项) ---

async fn list_config_sets(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
) -> Result<Json<ApiResponse<Vec<ConfigSetDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_config_sets(&actor.tenant_id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigSetBody {
    name: String,
    #[serde(default)]
    description: String,
}

async fn create_config_set(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<ConfigSetBody>,
) -> Result<Json<ApiResponse<ConfigSetDto>>, PlatformError> {
    let dto = state
        .service
        .create_config_set(&actor.tenant_id, &body.name, &body.description, &user.id)
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn update_config_set(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
    Json(body): Json<ConfigSetBody>,
) -> Result<Json<ApiResponse<ConfigSetDto>>, PlatformError> {
    let dto = state
        .service
        .update_config_set(&actor.tenant_id, &id, &body.name, &body.description)
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_config_set(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state.service.delete_config_set(&actor.tenant_id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn list_config_entries(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<ConfigEntryDto>>>, PlatformError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_config_entries(&actor.tenant_id, &id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutConfigEntryBody {
    key: String,
    /// Absent/empty = keep the stored value (same convention as the
    /// container/collaboration/SIEM secrets) — lets an admin flip the
    /// sensitive flag without re-pasting a credential.
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    sensitive: bool,
}

async fn put_config_entry(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
    Json(body): Json<PutConfigEntryBody>,
) -> Result<Json<ApiResponse<ConfigEntryDto>>, PlatformError> {
    let dto = state
        .service
        .put_config_entry(&actor.tenant_id, &id, &body.key, body.value.as_deref(), body.sensitive)
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_config_entry(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(entry_id): Path<String>,
) -> Result<Json<ApiResponse<()>>, PlatformError> {
    state.service.delete_config_entry(&actor.tenant_id, &entry_id).await?;
    Ok(Json(ApiResponse::ok(())))
}

/// Bulk import (P1-5 "Excel 批量迁移"). The frontend parses the CSV/Excel
/// file into rows (it already reads the file in the browser; the backend
/// stays free of spreadsheet-parsing deps) and posts the structured rows
/// here.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkImportConfigBody {
    #[serde(default)]
    rows: Vec<ConfigImportRow>,
    #[serde(default)]
    merge: bool,
}

async fn bulk_import_config_entries(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
    Json(body): Json<BulkImportConfigBody>,
) -> Result<Json<ApiResponse<ConfigBulkImportDto>>, PlatformError> {
    let dto = state
        .service
        .bulk_import_config_entries(&actor.tenant_id, &id, &body.rows, body.merge)
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

/// The reference report for one set: which skills embed
/// `{{config.<alias>.<key>}}` and how many — the number to check before
/// deleting or renaming a set.
async fn config_set_references(
    State(state): State<OnePlatformRouterState>,
    RequirePlatformAdmin(actor): RequirePlatformAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<ConfigSetReferencesDto>>, PlatformError> {
    let dto = state.service.config_set_references(&actor.tenant_id, &id).await?;
    Ok(Json(ApiResponse::ok(dto)))
}
