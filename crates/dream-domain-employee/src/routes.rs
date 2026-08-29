//! `/api/one/employee/*` routes. Mount behind the upstream auth middleware
//! (handlers read `CurrentUser` from request extensions; employees are
//! strictly owner-scoped in M3a).

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use dream_core_api_types::{ApiResponse, CronScheduleDto};
use dream_core_auth::CurrentUser;
use dream_core_common::ProviderWithModel;

use crate::error::EmployeeError;
use crate::models::{EmployeeGrantRow, EmployeeRunRow, PersonalAgentDto};
use crate::service::{CreateEmployeeInput, ScheduleInput, UpdateEmployeeInput};
use crate::state::OneEmployeeRouterState;

pub fn one_employee_routes(state: OneEmployeeRouterState) -> Router {
    Router::new()
        .route("/api/one/employee/agents", get(list_agents).post(create_agent))
        .route(
            "/api/one/employee/agents/{agent_id}",
            get(get_agent).put(update_agent).delete(delete_agent),
        )
        .route("/api/one/employee/agents/{agent_id}/run", post(run_agent))
        .route("/api/one/employee/agents/{agent_id}/run-team", post(run_agent_team))
        .route("/api/one/employee/agents/{agent_id}/schedule", put(set_schedule))
        .route("/api/one/employee/agents/{agent_id}/visibility", put(set_visibility))
        .route("/api/one/employee/agents/{agent_id}/runs", get(list_runs))
        .route("/api/one/employee/runs/{run_id}", get(get_run))
        // Admin registry + resource-authorization matrix (align-openocta §3,
        // delivery-gaps T4 step 2). Same admin gate devops uses for its own
        // registry writes (`require_registry_admin`) — direct SQL against
        // one-org's table, not a shared trait (see `EmployeeService::user_org_role`).
        .route("/api/one/employee/admin/agents", get(list_agents_for_admin))
        .route("/api/one/employee/admin/agents/publish", put(publish_agents))
        .route(
            "/api/one/employee/admin/grants",
            get(list_grants_for_admin)
                .put(grant_employee_access)
                .delete(revoke_employee_access),
        )
        // Content categories/tags (P1-1 round 1), shared across
        // skill/mcp/employee — see migration 007's own doc comment for why
        // this crate owns the tables even though the routes for skill/mcp
        // live in dream-domain-devops (which calls these service methods
        // directly, not via HTTP).
        .route(
            "/api/one/employee/admin/categories",
            get(list_categories).post(create_category),
        )
        .route(
            "/api/one/employee/admin/categories/{id}",
            put(update_category).delete(delete_category),
        )
        .route("/api/one/employee/admin/tags", get(list_tags).post(create_tag))
        .route("/api/one/employee/admin/tags/{id}", delete(delete_tag))
        .route(
            "/api/one/employee/admin/resource-tags",
            get(list_resource_tags).put(set_resource_tags),
        )
        .with_state(state)
}

async fn require_registry_admin(state: &OneEmployeeRouterState, user_id: &str) -> Result<(), EmployeeError> {
    match state.service.user_org_role(user_id).await? {
        None => Ok(()),
        Some(role) if role == "org_admin" || role == "system_admin" || role == "admin" => Ok(()),
        Some(_) => Err(EmployeeError::Forbidden(
            "the digital-employee registry and its authorization matrix are admin-only".into(),
        )),
    }
}

async fn list_agents(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<PersonalAgentDto>>>, EmployeeError> {
    // Own employees plus any shared within the caller's tenant (A1 L3).
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(
        state.service.list_available(&user.id, &tenant).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAgentBody {
    name: String,
    description: Option<String>,
    agent_type: String,
    custom_agent_id: Option<String>,
    cli_path: Option<String>,
    /// Persona / assistant definition id.
    assistant_id: Option<String>,
    /// `agent_metadata.id` to run the persona under, when the user overrode the
    /// backend the persona would otherwise imply.
    agent_id_override: Option<String>,
    /// Plain model id — ACP backends.
    model_id: Option<String>,
    /// dream only. `ProviderWithModel` has no `rename_all`, so its own keys
    /// stay snake_case (`provider_id` / `use_model`) despite this body being
    /// camelCase — same shape the frontend already sends for cron jobs.
    model: Option<ProviderWithModel>,
    automation_config: Option<serde_json::Value>,
}

async fn create_agent(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateAgentBody>,
) -> Result<Json<ApiResponse<PersonalAgentDto>>, EmployeeError> {
    // Tenant is the caller's org tenant (personal edition → 'default'); it
    // determines who a later-shared employee reaches (A1 L3).
    let tenant = state.tenant_of(&user.id).await;
    let agent = state
        .service
        .create(
            &user.id,
            &tenant,
            CreateEmployeeInput {
                name: body.name,
                description: body.description,
                agent_type: body.agent_type,
                custom_agent_id: body.custom_agent_id,
                cli_path: body.cli_path,
                assistant_id: body.assistant_id,
                agent_id_override: body.agent_id_override,
                model_id: body.model_id,
                model: body.model,
                automation_config: body.automation_config,
            },
        )
        .await?;
    Ok(Json(ApiResponse::ok(agent)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetVisibilityBody {
    visibility: String,
}

/// Share/unshare an employee within the owner's tenant. Owner-only.
async fn set_visibility(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(agent_id): Path<String>,
    Json(body): Json<SetVisibilityBody>,
) -> Result<Json<ApiResponse<PersonalAgentDto>>, EmployeeError> {
    let agent = state
        .service
        .set_visibility(&user.id, &agent_id, &body.visibility)
        .await?;
    Ok(Json(ApiResponse::ok(agent)))
}

async fn get_agent(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(agent_id): Path<String>,
) -> Result<Json<ApiResponse<PersonalAgentDto>>, EmployeeError> {
    Ok(Json(ApiResponse::ok(
        state.service.get(&user.id, &agent_id).await?.into(),
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
/// Omitted field → leave unchanged. For the nullable persona/model fields an
/// explicit `""` clears the column, so a client can detach a persona or model.
struct UpdateAgentBody {
    name: Option<String>,
    description: Option<String>,
    agent_type: Option<String>,
    assistant_id: Option<String>,
    agent_id_override: Option<String>,
    model_id: Option<String>,
    model: Option<ProviderWithModel>,
    automation_config: Option<serde_json::Value>,
}

async fn update_agent(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(agent_id): Path<String>,
    Json(body): Json<UpdateAgentBody>,
) -> Result<Json<ApiResponse<PersonalAgentDto>>, EmployeeError> {
    let tenant = state.tenant_of(&user.id).await;
    let agent = state
        .service
        .update(
            &user.id,
            &tenant,
            &agent_id,
            UpdateEmployeeInput {
                name: body.name,
                description: body.description,
                agent_type: body.agent_type,
                assistant_id: body.assistant_id,
                agent_id_override: body.agent_id_override,
                model_id: body.model_id,
                model: body.model,
                automation_config: body.automation_config,
            },
        )
        .await?;
    Ok(Json(ApiResponse::ok(agent)))
}

async fn delete_agent(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(agent_id): Path<String>,
) -> Result<Json<ApiResponse<()>>, EmployeeError> {
    state.service.delete(&user.id, &agent_id).await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunNowDto {
    run_id: String,
    conversation_id: String,
}

async fn run_agent(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(agent_id): Path<String>,
) -> Result<Json<ApiResponse<RunNowDto>>, EmployeeError> {
    let tenant = state.tenant_of(&user.id).await;
    let (run_id, conversation_id) = state.service.run_now(&user.id, &tenant, &agent_id).await?;
    Ok(Json(ApiResponse::ok(RunNowDto {
        run_id,
        conversation_id,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunTeamBody {
    team_id: String,
    slot_id: String,
}

async fn run_agent_team(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(agent_id): Path<String>,
    Json(body): Json<RunTeamBody>,
) -> Result<Json<ApiResponse<RunNowDto>>, EmployeeError> {
    let tenant = state.tenant_of(&user.id).await;
    let (run_id, conversation_id) = state
        .service
        .run_now_team(&user.id, &tenant, &agent_id, &body.team_id, &body.slot_id)
        .await?;
    Ok(Json(ApiResponse::ok(RunNowDto {
        run_id,
        conversation_id,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetScheduleBody {
    schedule: Option<CronScheduleDto>,
    #[serde(default)]
    enabled: Option<bool>,
}

async fn set_schedule(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(agent_id): Path<String>,
    Json(body): Json<SetScheduleBody>,
) -> Result<Json<ApiResponse<PersonalAgentDto>>, EmployeeError> {
    let tenant = state.tenant_of(&user.id).await;
    let agent = state
        .service
        .set_schedule(
            &user.id,
            &tenant,
            &agent_id,
            ScheduleInput {
                schedule: body.schedule,
                enabled: body.enabled,
            },
        )
        .await?;
    Ok(Json(ApiResponse::ok(agent)))
}

async fn list_runs(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(agent_id): Path<String>,
) -> Result<Json<ApiResponse<Vec<EmployeeRunRow>>>, EmployeeError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_runs(&user.id, &agent_id).await?,
    )))
}

async fn get_run(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(run_id): Path<String>,
) -> Result<Json<ApiResponse<EmployeeRunRow>>, EmployeeError> {
    Ok(Json(ApiResponse::ok(state.service.get_run(&user.id, &run_id).await?)))
}

// --- admin: registry + resource-authorization matrix ---

/// Every digital employee in the tenant, owner-agnostic — the admin
/// registry view (`ResourceRegistryTab`'s fourth tab on the dream-en side).
async fn list_agents_for_admin(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<PersonalAgentDto>>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(state.service.list_all_for_tenant(&tenant).await?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantSubjectQuery {
    subject_type: String,
    subject_id: String,
}

async fn list_grants_for_admin(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<GrantSubjectQuery>,
) -> Result<Json<ApiResponse<Vec<EmployeeGrantRow>>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(
        state
            .service
            .list_employee_grants(&tenant, &q.subject_type, &q.subject_id)
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantEmployeeAccessBody {
    subject_type: String,
    subject_id: String,
    employee_id: String,
    permission: String,
}

async fn grant_employee_access(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<GrantEmployeeAccessBody>,
) -> Result<Json<ApiResponse<()>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    state
        .service
        .grant_employee_access(
            &tenant,
            &body.subject_type,
            &body.subject_id,
            &body.employee_id,
            &body.permission,
            &user.id,
        )
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokeEmployeeAccessQuery {
    subject_type: String,
    subject_id: String,
    employee_id: String,
}

/// Query params, not a JSON body — this codebase's own convention for DELETE
/// (see `oneAdmin.deleteResourceGrant`'s path-param sibling on the dream-en
/// side); the frontend's `httpDelete` helper only builds URLs, never sends a
/// body.
async fn revoke_employee_access(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<RevokeEmployeeAccessQuery>,
) -> Result<Json<ApiResponse<()>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    state
        .service
        .revoke_employee_access(&tenant, &q.subject_type, &q.subject_id, &q.employee_id)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishBatchBody {
    ids: Vec<String>,
    published: bool,
}

async fn publish_agents(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<PublishBatchBody>,
) -> Result<Json<ApiResponse<()>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    state
        .service
        .set_published_batch(&tenant, &body.ids, body.published)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

// --- admin: content categories / tags (P1-1 round 1) ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceTypeQuery {
    resource_type: String,
}

async fn list_categories(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<ResourceTypeQuery>,
) -> Result<Json<ApiResponse<Vec<crate::models::ContentCategoryRow>>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(
        state.service.list_categories(&tenant, &q.resource_type).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCategoryBody {
    resource_type: String,
    parent_id: Option<String>,
    name: String,
    #[serde(default)]
    sort_order: i64,
}

async fn create_category(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateCategoryBody>,
) -> Result<Json<ApiResponse<crate::models::ContentCategoryRow>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    let row = state
        .service
        .create_category(
            &tenant,
            &body.resource_type,
            body.parent_id.as_deref(),
            &body.name,
            body.sort_order,
        )
        .await?;
    Ok(Json(ApiResponse::ok(row)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCategoryBody {
    parent_id: Option<String>,
    name: String,
    #[serde(default)]
    sort_order: i64,
}

async fn update_category(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateCategoryBody>,
) -> Result<Json<ApiResponse<crate::models::ContentCategoryRow>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let row = state
        .service
        .update_category(&id, body.parent_id.as_deref(), &body.name, body.sort_order)
        .await?;
    Ok(Json(ApiResponse::ok(row)))
}

async fn delete_category(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    state.service.delete_category(&id).await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn list_tags(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<ResourceTypeQuery>,
) -> Result<Json<ApiResponse<Vec<crate::models::ContentTagRow>>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(
        state.service.list_tags(&tenant, &q.resource_type).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTagBody {
    resource_type: String,
    name: String,
}

async fn create_tag(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateTagBody>,
) -> Result<Json<ApiResponse<crate::models::ContentTagRow>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    let row = state
        .service
        .create_tag(&tenant, &body.resource_type, &body.name)
        .await?;
    Ok(Json(ApiResponse::ok(row)))
}

async fn delete_tag(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    state.service.delete_tag(&id).await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceTagsQuery {
    resource_type: String,
    resource_id: String,
}

async fn list_resource_tags(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<ResourceTagsQuery>,
) -> Result<Json<ApiResponse<Vec<crate::models::ContentTagRow>>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    Ok(Json(ApiResponse::ok(
        state
            .service
            .list_resource_tags(&q.resource_type, &q.resource_id)
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetResourceTagsBody {
    resource_type: String,
    resource_id: String,
    tag_ids: Vec<String>,
}

async fn set_resource_tags(
    State(state): State<OneEmployeeRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<SetResourceTagsBody>,
) -> Result<Json<ApiResponse<()>>, EmployeeError> {
    require_registry_admin(&state, &user.id).await?;
    state
        .service
        .set_resource_tags(&body.resource_type, &body.resource_id, &body.tag_ids)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}
