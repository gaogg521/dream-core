//! `/api/one/*` memory routes — the enterprise memory subsystem (P2-2). The
//! prefix sits on the governance plane like one-workflow's, so the exact
//! same assembly both binaries serve and a later admin-svc split can lift
//! the admin half out whole.
//!
//! Member half: the collections a member can reach, their items, and a
//! readable-only content search. Admin half: the tenant's collection
//! inventory, refinement runs, and grant administration with the coverage
//! metric.

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use dream_core_api_types::ApiResponse;
use dream_core_auth::CurrentUser;

use crate::error::MemoryError;
use crate::models::{GrantCoverageDto, MemoryCollectionDto, MemoryGrantDto, MemoryItemDto, MemoryRefineJobDto};
use crate::rbac::{RequireMemoryAdmin, RequireMemoryMember};
use crate::state::OneMemoryRouterState;

pub fn one_memory_routes(state: OneMemoryRouterState) -> Router {
    Router::new()
        .route(
            "/api/one/admin/memory/collections",
            get(admin_list_collections).post(admin_create_collection),
        )
        .route(
            "/api/one/admin/memory/collections/{id}",
            put(admin_update_collection).delete(admin_delete_collection),
        )
        .route(
            "/api/one/admin/memory/collections/{id}/refine",
            post(admin_refine_collection),
        )
        .route("/api/one/admin/memory/collections/{id}/grants", get(admin_list_grants))
        .route("/api/one/admin/memory/grants", put(admin_put_grant))
        .route("/api/one/admin/memory/grants/{grantId}", delete(admin_revoke_grant))
        .route("/api/one/admin/memory/coverage", get(admin_coverage))
        .route("/api/one/memory/collections", get(member_list_collections))
        .route(
            "/api/one/memory/collections/{id}/items",
            get(member_list_items).post(member_add_item),
        )
        .route("/api/one/memory/search", get(member_search))
        .with_state(state)
}

// ---------- admin half ----------

async fn admin_list_collections(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryAdmin(actor): RequireMemoryAdmin,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<MemoryCollectionDto>>>, MemoryError> {
    Ok(Json(ApiResponse::ok(
        state
            .service
            .list_collections(&actor.tenant_id, &user.id, &actor.role)
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCollectionBody {
    /// One of [`crate::service::MEMORY_SCOPES`].
    scope: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    department_id: Option<String>,
    #[serde(default)]
    owner_user_id: Option<String>,
}

async fn admin_create_collection(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryAdmin(actor): RequireMemoryAdmin,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateCollectionBody>,
) -> Result<Json<ApiResponse<MemoryCollectionDto>>, MemoryError> {
    let dto = state
        .service
        .create_collection(
            &actor.tenant_id,
            &user.id,
            &actor.role,
            &body.scope,
            body.department_id.as_deref(),
            body.owner_user_id.as_deref(),
            &body.name,
            body.description.as_deref().unwrap_or(""),
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCollectionBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

async fn admin_update_collection(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryAdmin(actor): RequireMemoryAdmin,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateCollectionBody>,
) -> Result<Json<ApiResponse<MemoryCollectionDto>>, MemoryError> {
    let dto = state
        .service
        .update_collection(
            &actor.tenant_id,
            &user.id,
            &actor.role,
            &id,
            body.name.as_deref(),
            body.description.as_deref(),
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn admin_delete_collection(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryAdmin(actor): RequireMemoryAdmin,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, MemoryError> {
    state
        .service
        .delete_collection(&actor.tenant_id, &user.id, &actor.role, &id)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn admin_refine_collection(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryAdmin(actor): RequireMemoryAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<MemoryRefineJobDto>>, MemoryError> {
    let dto = state.service.run_refine_job(&actor.tenant_id, &id).await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn admin_list_grants(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryAdmin(actor): RequireMemoryAdmin,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<MemoryGrantDto>>>, MemoryError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_grants(&actor.tenant_id, &id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutGrantBody {
    collection_id: String,
    /// `"member" | "department"`.
    subject_type: String,
    subject_id: String,
    /// `"read" | "write"`.
    access: String,
}

async fn admin_put_grant(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryAdmin(actor): RequireMemoryAdmin,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<PutGrantBody>,
) -> Result<Json<ApiResponse<MemoryGrantDto>>, MemoryError> {
    let dto = state
        .service
        .grant_memory(
            &actor.tenant_id,
            &body.collection_id,
            &body.subject_type,
            &body.subject_id,
            &body.access,
            &user.id,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn admin_revoke_grant(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryAdmin(actor): RequireMemoryAdmin,
    Path(grant_id): Path<String>,
) -> Result<Json<ApiResponse<()>>, MemoryError> {
    state.service.revoke_memory(&actor.tenant_id, &grant_id).await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn admin_coverage(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryAdmin(actor): RequireMemoryAdmin,
) -> Result<Json<ApiResponse<GrantCoverageDto>>, MemoryError> {
    Ok(Json(ApiResponse::ok(
        state.service.grant_coverage(&actor.tenant_id).await?,
    )))
}

// ---------- member half ----------

async fn member_list_collections(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryMember(actor): RequireMemoryMember,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<MemoryCollectionDto>>>, MemoryError> {
    Ok(Json(ApiResponse::ok(
        state
            .service
            .list_collections(&actor.tenant_id, &user.id, &actor.role)
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListItemsQuery {
    #[serde(default)]
    limit: Option<i64>,
}

async fn member_list_items(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryMember(actor): RequireMemoryMember,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Query(query): Query<ListItemsQuery>,
) -> Result<Json<ApiResponse<Vec<MemoryItemDto>>>, MemoryError> {
    Ok(Json(ApiResponse::ok(
        state
            .service
            .list_items(&actor.tenant_id, &user.id, &actor.role, &id, query.limit.unwrap_or(200))
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddItemBody {
    content: String,
    #[serde(default = "default_importance")]
    importance: f64,
    #[serde(default)]
    source_conversation_id: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn default_importance() -> f64 {
    0.5
}

async fn member_add_item(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryMember(actor): RequireMemoryMember,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<AddItemBody>,
) -> Result<Json<ApiResponse<MemoryItemDto>>, MemoryError> {
    let dto = state
        .service
        .add_item(
            &actor.tenant_id,
            &user.id,
            &actor.role,
            &id,
            &body.content,
            body.importance,
            body.source_conversation_id.as_deref(),
            &body.tags,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchItemsQuery {
    query: String,
    #[serde(default)]
    collection_id: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn member_search(
    State(state): State<OneMemoryRouterState>,
    RequireMemoryMember(actor): RequireMemoryMember,
    Extension(user): Extension<CurrentUser>,
    Query(query): Query<SearchItemsQuery>,
) -> Result<Json<ApiResponse<Vec<MemoryItemDto>>>, MemoryError> {
    Ok(Json(ApiResponse::ok(
        state
            .service
            .search_items(
                &actor.tenant_id,
                &user.id,
                &actor.role,
                &query.query,
                query.collection_id.as_deref(),
                query.limit.unwrap_or(50),
            )
            .await?,
    )))
}
