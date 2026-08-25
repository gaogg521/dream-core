//! `/api/one/admin/platform/*` routes — deployment infrastructure config
//! (P1-3 container runtime + P2-2 realtime collaboration), the E5
//! resource-authorization matrix (`resource-grants*`), and E5 scene
//! management (`scenes*`) — a named bundle of resource grants a member gets
//! in one action by joining the scene, instead of an admin granting each
//! skill/tool/model/channel one at a time.
//!
//! Mounted behind the upstream `auth_middleware` (relies on `CurrentUser` in
//! request extensions). All routes are gated by `RequirePlatformAdmin`.
//!
//! ⚠️ The resource-grants endpoints let an admin record who may reach which
//! skill / MCP server / digital employee / model channel, but nothing reads
//! `PlatformService::effective_resource_ids` on the enforcement path yet — the
//! four devops registries still gate purely on their own `scope`/`visibility`
//! columns. This is the storage + resolution layer landing first because it's
//! the part later work can't safely retrofit; wiring an actual check into
//! each of the four resource kinds is separate, follow-up work.

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use dream_core_api_types::ApiResponse;
use dream_core_auth::CurrentUser;

use crate::collaboration::CollaborationStatus;
use crate::container::ContainerStatus;
use crate::error::PlatformError;
use crate::models::{
    CollaborationConfigDto, ContainerConfigDto, EffectiveGrantDto, IpAllowlistConfigDto, ResourceGrantDto, SceneDto,
    SiemConfigDto,
};
use crate::rbac::RequirePlatformAdmin;
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
/// own grants and their department chain. Admin-gated for now, same as every
/// other route here — a self-service "what can I see" endpoint for a caller
/// to ask about themselves is a straightforward follow-up once something
/// actually enforces this matrix, but nothing does yet (see the module docs).
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
