//! `/api/workflow/*` routes — the approval workflow subsystem (P2-1). The
//! prefix mirrors the reference product's dedicated workflow service; here
//! the routes mount on the governance plane, so the exact same assembly both
//! binaries serve and a later admin-svc split can lift them out whole.
//!
//! Member half: submit a task, watch own submissions. Admin half (the
//! "approval group"): the pending queue (待办), the decided history (已办),
//! and every decision.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use dream_core_api_types::ApiResponse;
use dream_core_auth::CurrentUser;

use crate::error::WorkflowError;
use crate::models::WorkflowTaskDto;
use crate::rbac::{RequireWorkflowAdmin, RequireWorkflowMember};
use crate::state::OneWorkflowRouterState;

pub fn one_workflow_routes(state: OneWorkflowRouterState) -> Router {
    Router::new()
        .route("/api/workflow/tasks", get(list_tasks).post(create_task))
        .route("/api/workflow/tasks/{id}", get(get_task))
        .route("/api/workflow/tasks/{id}/decision", post(decide_task))
        .with_state(state)
}

async fn get_task(
    State(state): State<OneWorkflowRouterState>,
    RequireWorkflowMember(actor): RequireWorkflowMember,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<WorkflowTaskDto>>, WorkflowError> {
    let task = state
        .service
        .get_task(&actor.tenant_id, &id)
        .await?
        .ok_or_else(|| WorkflowError::NotFound("workflow task not found".into()))?;
    Ok(Json(ApiResponse::ok(task)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListTasksQuery {
    /// `"pending" | "decided" | "mine"`. Defaults to the admin queue's
    /// `pending`; `mine` always scopes to the caller.
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn list_tasks(
    State(state): State<OneWorkflowRouterState>,
    RequireWorkflowMember(member): RequireWorkflowMember,
    Extension(user): Extension<CurrentUser>,
    Query(query): Query<ListTasksQuery>,
) -> Result<Json<ApiResponse<Vec<WorkflowTaskDto>>>, WorkflowError> {
    // The pending and decided views are the approvers' (待办/已办); `mine` is
    // self-scoped for any member. Which gate applies depends on the query
    // string, which extractors run ahead of — so the member gate sits on the
    // route and the admin gate is enforced here, in the same service, with
    // the same answer it would have given from an extractor.
    let view = query.view.as_deref().unwrap_or("pending");
    let (tenant_id, requester_id) = match view {
        "mine" => (member.tenant_id, Some(user.id.clone())),
        "pending" | "decided" => {
            let admin = state.service.require_admin(&user.id).await?;
            (admin.tenant_id, None)
        }
        other => return Err(WorkflowError::BadRequest(format!("unknown task view '{other}'"))),
    };
    Ok(Json(ApiResponse::ok(
        state
            .service
            .list_tasks(&tenant_id, view, requester_id.as_deref(), query.limit.unwrap_or(200))
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTaskBody {
    /// One of `WORKFLOW_TASK_KINDS`.
    kind: String,
    title: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

async fn create_task(
    State(state): State<OneWorkflowRouterState>,
    RequireWorkflowMember(actor): RequireWorkflowMember,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateTaskBody>,
) -> Result<Json<ApiResponse<WorkflowTaskDto>>, WorkflowError> {
    let dto = state
        .service
        .create_task(
            &actor.tenant_id,
            &body.kind,
            &user.id,
            &body.title,
            body.detail.as_deref().unwrap_or(""),
            body.payload.as_ref().unwrap_or(&serde_json::Value::Null),
            // Member-submitted tasks never time out — they wait for a human.
            None,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecideTaskBody {
    /// `"approved" | "rejected"`.
    decision: String,
    #[serde(default)]
    note: Option<String>,
}

async fn decide_task(
    State(state): State<OneWorkflowRouterState>,
    RequireWorkflowAdmin(actor): RequireWorkflowAdmin,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<DecideTaskBody>,
) -> Result<Json<ApiResponse<WorkflowTaskDto>>, WorkflowError> {
    let dto = state
        .service
        .decide(&actor.tenant_id, &id, &body.decision, &user.id, body.note.as_deref())
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}
