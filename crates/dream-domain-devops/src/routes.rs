//! `/api/one/devops/*` routes. Mount behind the upstream auth middleware —
//! the whole board is collaborative, so every authenticated org member can
//! read and write (matching the 1one superAssistant behavior).

use axum::extract::{Path, Query, State};
use axum::routing::{get, patch};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use dream_core_api_types::ApiResponse;
use dream_core_auth::CurrentUser;

use crate::api_assets::{ApiAssetDetailDto, ApiAssetDto};
use crate::dlp_service::{DlpEventDto, DlpEventInput, DlpRuleDto, DlpSummaryDto};
use crate::error::DevopsError;
use crate::market_sync::{MarketSourceDto, MarketSyncReportDto};
use crate::models::{
    McpRegistryDto, MilestoneDto, PipelineDto, PipelineRunDto, ProviderChannelDto, RagConfigDto, RagDocumentDto,
    RagSearchHit, RequirementCommentDto, RequirementDto, SkillRegistryDto, TestCaseDto, TestPlanDto,
};
use crate::service::{CreateRequirementInput, UpdateRequirementInput};
use crate::state::OneDevopsRouterState;

pub fn one_devops_routes(state: OneDevopsRouterState) -> Router {
    Router::new()
        .route("/api/one/devops/requirements/tree", get(requirements_tree))
        .route("/api/one/devops/requirements", axum::routing::post(create_requirement))
        .route(
            "/api/one/devops/requirements/{id}",
            patch(update_requirement).delete(delete_requirement),
        )
        .route(
            "/api/one/devops/requirements/{id}/comments",
            get(list_comments).post(create_comment),
        )
        .route(
            "/api/one/devops/requirements/{id}/dispatch",
            axum::routing::post(dispatch_requirement),
        )
        .route(
            "/api/one/devops/requirements/{id}/breakdown",
            axum::routing::post(breakdown_requirement),
        )
        .route("/api/one/devops/skills", get(list_skills).post(upsert_skill))
        .route("/api/one/devops/skills/{id}", axum::routing::delete(delete_skill))
        // P1-1 round 1: batch publish/unpublish + upload-a-SKILL.md.
        .route("/api/one/devops/skills/publish", axum::routing::put(publish_skills))
        .route("/api/one/devops/skills/upload", axum::routing::post(upload_skill))
        // P1-6 API assets: imported Swagger/OpenAPI docs, publishable into the
        // skill registry so member agents can call the endpoints via curl.
        .route(
            "/api/one/devops/api-assets",
            get(list_api_assets).post(import_api_asset),
        )
        .route(
            "/api/one/devops/api-assets/{id}",
            get(get_api_asset).delete(delete_api_asset),
        )
        .route(
            "/api/one/devops/api-assets/{id}/publish",
            axum::routing::post(publish_api_asset),
        )
        // P1-1 round 2: remote content market — admin-curated HTTP(S)
        // sources synced into the skill/MCP registries (origin='market').
        .route(
            "/api/one/devops/market/sources",
            get(list_market_sources).post(create_market_source),
        )
        .route(
            "/api/one/devops/market/sources/{id}",
            axum::routing::put(update_market_source).delete(delete_market_source),
        )
        .route(
            "/api/one/devops/market/sources/{id}/sync",
            axum::routing::post(sync_market_source),
        )
        .route("/api/one/devops/mcp-registry", get(list_mcp).post(upsert_mcp))
        .route("/api/one/devops/mcp-registry/{id}", axum::routing::delete(delete_mcp))
        .route("/api/one/devops/mcp-registry/publish", axum::routing::put(publish_mcp))
        .route(
            "/api/one/devops/model-channels",
            get(list_model_channels).post(upsert_model_channel),
        )
        .route(
            "/api/one/devops/model-channels/{id}",
            axum::routing::delete(delete_model_channel),
        )
        .route(
            "/api/one/devops/model-channels/{id}/token",
            axum::routing::post(issue_model_channel_token),
        )
        // Content inspection (T4). Rules are admin-authored; the member-facing
        // list is deliberately readable by any member, because enforcement runs
        // on their machine and a rule they cannot fetch is a rule that silently
        // does nothing.
        .route("/api/one/devops/dlp/rules", get(list_dlp_rules).post(upsert_dlp_rule))
        .route("/api/one/devops/dlp/rules/{id}", axum::routing::delete(delete_dlp_rule))
        .route("/api/one/devops/dlp/my-rules", get(list_my_dlp_rules))
        .route(
            "/api/one/devops/dlp/events",
            get(list_dlp_events).post(report_dlp_events),
        )
        // Aggregated findings for the reports' security half — same admin gate
        // as the raw list it summarizes.
        .route("/api/one/devops/dlp/summary", get(dlp_summary))
        .route("/api/one/devops/rag/documents", get(list_rag).post(register_rag))
        .route("/api/one/devops/rag/documents/{id}", axum::routing::delete(delete_rag))
        .route(
            "/api/one/devops/rag/documents/{id}/content",
            axum::routing::put(set_rag_content),
        )
        .route(
            "/api/one/devops/rag/documents/{id}/process",
            axum::routing::post(process_rag),
        )
        .route("/api/one/devops/rag/config", get(get_rag_config).put(set_rag_config))
        .route("/api/one/devops/rag/search", axum::routing::post(search_rag))
        // P1-2 offboarding: inspect and hand over a departing member's assets.
        .route("/api/one/devops/ownership/{user_id}/count", get(count_owned_resources))
        .route(
            "/api/one/devops/ownership/transfer",
            axum::routing::post(transfer_ownership),
        )
        .route(
            "/api/one/devops/milestones",
            get(list_milestones).post(create_milestone),
        )
        .route(
            "/api/one/devops/milestones/{id}",
            patch(update_milestone).delete(delete_milestone),
        )
        // test plans (A4)
        .route(
            "/api/one/devops/test-plans",
            get(list_test_plans).post(create_test_plan),
        )
        .route(
            "/api/one/devops/test-plans/{id}",
            patch(update_test_plan).delete(delete_test_plan),
        )
        .route(
            "/api/one/devops/test-plans/{id}/cases",
            get(list_test_cases).post(create_test_case),
        )
        .route(
            "/api/one/devops/test-plans/{plan_id}/cases/{id}",
            patch(update_test_case).delete(delete_test_case),
        )
        // pipelines (A4)
        .route("/api/one/devops/pipelines", get(list_pipelines).post(create_pipeline))
        .route(
            "/api/one/devops/pipelines/{id}",
            patch(update_pipeline).delete(delete_pipeline),
        )
        .route(
            "/api/one/devops/pipelines/{id}/runs",
            get(list_pipeline_runs).post(create_pipeline_run),
        )
        .route(
            "/api/one/devops/pipelines/{pipeline_id}/runs/{id}",
            patch(update_pipeline_run),
        )
        .with_state(state)
}

/// Governance-only subset of `/api/one/devops/*`, mounted by the standalone
/// `dreamcore-admin` binary alongside org/enterprise/billing/platform/sso.
///
/// Every other devops route (requirements, skills, mcp-registry, rag,
/// milestones, test-plans, pipelines) has no admin-console caller — the
/// per-route front-end audit behind this split found the admin console only
/// ever calls DLP rule authoring, model-channel deletion, and offboarding
/// ownership transfer (see dream-en's docs/roadmap.zh-CN.md, E1.5) — so they
/// stay exclusive to `one_devops_routes` on the personal-workbench process.
pub fn admin_devops_routes(state: OneDevopsRouterState) -> Router {
    Router::new()
        .route("/api/one/devops/dlp/rules", get(list_dlp_rules).post(upsert_dlp_rule))
        .route("/api/one/devops/dlp/rules/{id}", axum::routing::delete(delete_dlp_rule))
        .route(
            "/api/one/devops/model-channels/{id}",
            axum::routing::delete(delete_model_channel),
        )
        .route("/api/one/devops/ownership/{user_id}/count", get(count_owned_resources))
        .route(
            "/api/one/devops/ownership/transfer",
            axum::routing::post(transfer_ownership),
        )
        .with_state(state)
}

// -- requirements ---------------------------------------------------------

async fn requirements_tree(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<RequirementDto>>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(state.service.requirements_tree(&tenant).await?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRequirementBody {
    subject: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    milestone_id: Option<String>,
    #[serde(default)]
    autopilot: Option<bool>,
}

async fn create_requirement(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateRequirementBody>,
) -> Result<Json<ApiResponse<RequirementDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let created = state
        .service
        .create_requirement(
            &tenant,
            &user.id,
            Some(user.username.as_str()),
            CreateRequirementInput {
                parent_id: body.parent_id,
                kind: body.kind,
                subject: body.subject,
                description: body.description,
                priority: body.priority,
                milestone_id: body.milestone_id,
                autopilot: body.autopilot,
            },
        )
        .await?;
    maybe_autopilot(&state, &user.id, &created.id).await;
    Ok(Json(ApiResponse::ok(created)))
}

/// PATCH body: absent field = keep, `null` = clear (for nullable columns).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateRequirementBody {
    #[serde(default)]
    subject: Option<String>,
    #[serde(default, with = "double_option")]
    description: Option<Option<String>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default, with = "double_option")]
    assigned_to: Option<Option<String>>,
    #[serde(default, with = "double_option")]
    parent_id: Option<Option<String>>,
    #[serde(default, with = "double_option")]
    milestone_id: Option<Option<String>>,
    #[serde(default)]
    autopilot: Option<bool>,
}

/// serde helper distinguishing "absent" from "null".
mod double_option {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

async fn update_requirement(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRequirementBody>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    state
        .service
        .update_requirement(
            &tenant,
            &id,
            UpdateRequirementInput {
                subject: body.subject,
                description: body.description,
                status: body.status,
                priority: body.priority,
                assigned_to: body.assigned_to,
                parent_id: body.parent_id,
                milestone_id: body.milestone_id,
                autopilot: body.autopilot,
            },
        )
        .await?;
    maybe_autopilot(&state, &user.id, &id).await;
    Ok(Json(ApiResponse::ok(())))
}

async fn delete_requirement(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    state.service.delete_requirement(&tenant, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn list_comments(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<RequirementCommentDto>>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(state.service.list_comments(&tenant, &id).await?)))
}

#[derive(Deserialize)]
struct CreateCommentBody {
    body: String,
}

async fn create_comment(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<CreateCommentBody>,
) -> Result<Json<ApiResponse<RequirementCommentDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let created = state
        .service
        .create_comment(&tenant, &id, &user.id, &user.username, &body.body)
        .await?;
    Ok(Json(ApiResponse::ok(created)))
}

// -- orchestration (A1 dispatch) ------------------------------------------

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DispatchResult {
    conversation_id: String,
    run_id: String,
}

/// Dispatch a requirement to its assigned digital employee: run the employee
/// with the requirement as task context, record the run linkage as an
/// agent-authored comment, and advance the status to `developing`.
///
/// L1 constraint: `assigned_to` must be one of the caller's own personal
/// digital employees (one-employee enforces owner isolation inside
/// `run_now_with_context`). Team-shared employees are a later layer.
async fn dispatch_requirement(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<DispatchResult>>, DevopsError> {
    let result = dispatch_core(&state, &user.id, &id).await?;
    Ok(Json(ApiResponse::ok(result)))
}

/// Core dispatch: run the requirement's assigned digital employee with the
/// requirement as task context, record the run linkage as an agent comment,
/// and advance a pre-dev status to `developing`. Shared by the manual
/// dispatch endpoint and autopilot. Errors with `BadRequest` when the
/// requirement has no assigned employee.
async fn dispatch_core(state: &OneDevopsRouterState, user_id: &str, id: &str) -> Result<DispatchResult, DevopsError> {
    let employee = state
        .employee
        .as_ref()
        .ok_or_else(|| DevopsError::Internal("employee runtime not wired".into()))?;

    let tenant = state.tenant_of(user_id).await;
    let req = state.service.get_requirement_row(&tenant, id).await?;
    let assigned_to = req
        .assigned_to
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| DevopsError::BadRequest("requirement has no assigned digital employee".into()))?;

    let mut task_context = build_task_context(&req);

    // M2-RAG: enrich the employee's task with team knowledge. Strictly
    // best-effort — RAG unconfigured, embedding endpoint down, or an empty
    // index must never block a dispatch (and standalone mode has no RAG).
    let rag_query = format!("{} {}", req.subject, req.description.as_deref().unwrap_or(""));
    // Scope retrieval to what the dispatching user may see (P0-4 ACL).
    if let Ok(hits) = state.service.search_rag(user_id, &rag_query, 3).await {
        let relevant: Vec<_> = hits.into_iter().filter(|h| h.score >= 0.35).collect();
        if !relevant.is_empty() {
            task_context.push_str("\n\n——团队知识库参考（自动检索，按相关度）——\n");
            for hit in &relevant {
                task_context.push_str(&format!("\n【{}】\n{}\n", hit.document_title, hit.content));
            }
        }
    }

    // Atomically claim the pre-dev → developing transition BEFORE the
    // quota-costing run so a concurrent manual dispatch + autopilot (or a
    // double dispatch) can't both fire. A requirement already past pre-dev is
    // a deliberate re-dispatch of an in-progress item — allowed, no claim.
    let was_pre_dev = req.status == "backlog" || req.status == "planning";
    if was_pre_dev && !state.service.claim_requirement_for_dispatch(&tenant, id).await? {
        return Err(DevopsError::BadRequest(
            "该需求正在被派发（并发抢占已被另一次调用赢得），请勿重复派发".into(),
        ));
    }

    let run = employee
        .run_now_with_context(user_id, &tenant, assigned_to, task_context)
        .await;
    let (run_id, conversation_id) = match run {
        Ok(v) => v,
        Err(e) => {
            // The run never started, so roll the status back to where it was
            // instead of leaving the requirement stuck in `developing`.
            if was_pre_dev {
                let _ = state
                    .service
                    .update_requirement(
                        &tenant,
                        id,
                        UpdateRequirementInput {
                            status: Some(req.status.clone()),
                            ..Default::default()
                        },
                    )
                    .await;
            }
            return Err(match e {
                dream_domain_employee::EmployeeError::NotFound => DevopsError::BadRequest(
                    "assigned digital employee is not available to you (not your employee, and not shared within your team)".into(),
                ),
                other => DevopsError::Internal(format!("dispatch run: {other}")),
            });
        }
    };

    let metadata = serde_json::json!({ "conversationId": conversation_id, "runId": run_id }).to_string();
    let body = format!("已派发给数字员工，运行中（会话 {conversation_id}）");
    state
        .service
        .insert_agent_comment(
            &tenant,
            id,
            "agent",
            Some(assigned_to),
            "数字员工",
            &body,
            Some(metadata),
        )
        .await?;
    state
        .service
        .audit(&tenant, user_id, "devops.requirement.dispatch", Some(id), None)
        .await;

    // Status was already advanced to `developing` by the atomic claim above
    // (for pre-dev requirements) before the run started.

    Ok(DispatchResult {
        conversation_id,
        run_id,
    })
}

/// Best-effort autopilot (A1 L3): after a create/update, if the requirement
/// has autopilot on, an assigned employee, and is still in a pre-dev status,
/// auto-dispatch it. Silent no-op when conditions aren't met; failures are
/// logged, never surfaced — autopilot must not fail the originating request.
///
/// Re-entrancy is self-guarding: a successful dispatch advances the status to
/// `developing`, so the `backlog`/`planning` gate stops it from firing again
/// until the user deliberately moves the requirement back.
async fn maybe_autopilot(state: &OneDevopsRouterState, user_id: &str, id: &str) {
    if state.employee.is_none() {
        return;
    }
    let tenant = state.tenant_of(user_id).await;
    let Ok(req) = state.service.get_requirement_row(&tenant, id).await else {
        return;
    };
    if !req.autopilot {
        return;
    }
    if req
        .assigned_to
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return;
    }
    if req.status != "backlog" && req.status != "planning" {
        return;
    }
    if let Err(e) = dispatch_core(state, user_id, id).await {
        tracing::warn!(requirement = id, error = %e, "autopilot dispatch failed");
    }
}

/// Best-effort — same "skip when the employee runtime isn't wired" idiom as
/// `maybe_autopilot` above (unit tests build `OneDevopsRouterState` without
/// it; production always wires it). The category/tag tables live in
/// `dream-domain-employee` (P1-1 round 1, migration 007) — this crate
/// already has a real Cargo dependency on it, so this is a plain call, not
/// a cross-crate SQL read.
async fn set_resource_tags_if_wired(
    state: &OneDevopsRouterState,
    resource_type: &str,
    resource_id: &str,
    tag_ids: &[String],
) -> Result<(), DevopsError> {
    let Some(employee) = &state.employee else {
        return Ok(());
    };
    employee
        .set_resource_tags(resource_type, resource_id, tag_ids)
        .await
        .map_err(|e| DevopsError::Internal(e.to_string()))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BreakdownResult {
    conversation_id: String,
    run_id: String,
    created: Vec<RequirementDto>,
}

/// Break a requirement down into child requirements (A1 L2): run the assigned
/// digital employee with a structured breakdown prompt, parse its reply into
/// child items, batch-create them under this requirement, and record the run
/// linkage as an agent-authored comment.
///
/// Same L1 ownership constraint as dispatch: `assigned_to` must be one of the
/// caller's own personal digital employees.
async fn breakdown_requirement(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<BreakdownResult>>, DevopsError> {
    let employee = state
        .employee
        .as_ref()
        .ok_or_else(|| DevopsError::Internal("employee runtime not wired".into()))?;

    let tenant = state.tenant_of(&user.id).await;
    let req = state.service.get_requirement_row(&tenant, &id).await?;
    let assigned_to = req
        .assigned_to
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| DevopsError::BadRequest("requirement has no assigned digital employee".into()))?;

    let prompt = crate::breakdown::build_breakdown_prompt(&req);
    let run = employee
        .run_prompt_blocking(&user.id, &tenant, assigned_to, prompt)
        .await
        .map_err(|e| match e {
            dream_domain_employee::EmployeeError::NotFound => DevopsError::BadRequest(
                "assigned digital employee is not available to you (not your employee, and not shared within your team)".into(),
            ),
            other => DevopsError::Internal(format!("breakdown run: {other}")),
        })?;

    let items = crate::breakdown::parse_breakdown_items(&run.reply);
    if items.is_empty() {
        // Record the failure so the run linkage is not lost, then surface it.
        let metadata = serde_json::json!({ "conversationId": run.conversation_id, "runId": run.run_id }).to_string();
        state
            .service
            .insert_agent_comment(
                &tenant,
                &id,
                "agent",
                Some(assigned_to),
                "数字员工",
                "自动拆解未能从回复中解析出子需求，请重试或手动拆解。",
                Some(metadata),
            )
            .await?;
        return Err(DevopsError::BadRequest("未能从数字员工回复中解析出子需求".into()));
    }

    let created = state
        .service
        .create_breakdown_children(&tenant, &id, &user.id, Some(user.username.as_str()), &items)
        .await?;

    let child_ids: Vec<&str> = created.iter().map(|c| c.id.as_str()).collect();
    let metadata = serde_json::json!({
        "conversationId": run.conversation_id,
        "runId": run.run_id,
        "childIds": child_ids,
    })
    .to_string();
    let body = format!(
        "已自动拆解为 {} 条子需求（会话 {}）",
        created.len(),
        run.conversation_id
    );
    state
        .service
        .insert_agent_comment(
            &tenant,
            &id,
            "agent",
            Some(assigned_to),
            "数字员工",
            &body,
            Some(metadata),
        )
        .await?;

    state
        .service
        .audit(&tenant, &user.id, "devops.requirement.breakdown", Some(&id), None)
        .await;
    Ok(Json(ApiResponse::ok(BreakdownResult {
        conversation_id: run.conversation_id,
        run_id: run.run_id,
        created,
    })))
}

/// Compose the requirement into a task prompt appended to the employee's own
/// run prompt. Kept plain-text so any agent backend can consume it.
fn build_task_context(req: &crate::models::RequirementRow) -> String {
    let mut out = String::new();
    out.push_str("你收到一条协作看板需求，请完成它并输出可交付摘要。\n\n");
    out.push_str(&format!("标题：{}\n", req.subject));
    out.push_str(&format!("类型：{} · 优先级：{}\n", req.r#type, req.priority));
    if let Some(desc) = req.description.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(&format!("\n描述：\n{desc}\n"));
    }
    out
}

// -- registries -----------------------------------------------------------

/// Distribution-policy gate: registry WRITES (skills / MCP / RAG) define what
/// gets distributed to every member's machine, so inside an enterprise they
/// are admin-only. A user with no org row (standalone / personal mode, or a
/// member's own local backend) is the machine owner and passes.
///
/// Reads (list/search) and collaboration surfaces (requirements / comments /
/// dispatch / milestones / test plans / pipelines) stay member-open.
/// Best-effort audit for a policy-changing action (D6). Resolves the caller's
/// tenant and records the action; never fails the request. These call sites
/// fire only after the operation returned Ok, so they are all successes; the
/// registry one-liners are near-instant and pass no `latency_ms`.
async fn audit(state: &OneDevopsRouterState, user_id: &str, action: &str, resource: Option<&str>) {
    let tenant = state.tenant_of(user_id).await;
    state.service.audit(&tenant, user_id, action, resource, None).await;
}

async fn require_registry_admin(state: &OneDevopsRouterState, user_id: &str) -> Result<(), DevopsError> {
    match state.service.user_org_role(user_id).await? {
        None => Ok(()),
        Some(role) if role == "org_admin" || role == "system_admin" || role == "admin" => Ok(()),
        Some(_) => Err(DevopsError::Forbidden(
            "registry writes are admin-only: distributed skills/MCP/knowledge affect every member".into(),
        )),
    }
}

async fn list_skills(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<SkillRegistryDto>>>, DevopsError> {
    Ok(Json(ApiResponse::ok(state.service.list_skills(&user.id).await?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertSkillBody {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    content: String,
    #[serde(default = "default_true")]
    enabled: bool,
    /// Mixed distribution model: admin marks the skill as auto-active for
    /// member agents. Defaults to opt-in (false).
    #[serde(default)]
    auto_active: bool,
    /// P0-4 read ACL: `'org'` (whole enterprise) or `'team'` (a project group).
    #[serde(default = "default_scope_org")]
    scope: String,
    /// Project group id when scope is `'team'`.
    #[serde(default)]
    team_id: Option<String>,
    /// `'all'` (every member in scope) or `'admin'` (admins only).
    #[serde(default = "default_visibility_all")]
    visibility: String,
    /// P1-1 round 1. Both optional and orthogonal to scope/visibility.
    #[serde(default)]
    category_id: Option<String>,
    #[serde(default)]
    tag_ids: Option<Vec<String>>,
}

fn default_true() -> bool {
    true
}

fn default_scope_org() -> String {
    "org".into()
}

fn default_visibility_all() -> String {
    "all".into()
}

async fn upsert_skill(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<UpsertSkillBody>,
) -> Result<Json<ApiResponse<SkillRegistryDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let dto = state
        .service
        .upsert_skill(
            body.id.as_deref(),
            &body.name,
            &body.description,
            &body.content,
            body.enabled,
            body.auto_active,
            &body.scope,
            body.team_id.as_deref(),
            &body.visibility,
            body.category_id.as_deref(),
            &user.id,
        )
        .await?;
    if let Some(tag_ids) = &body.tag_ids {
        set_resource_tags_if_wired(&state, "skill", &dto.id, tag_ids).await?;
    }
    audit(&state, &user.id, "devops.skill.upsert", Some(&dto.id)).await;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_skill(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    audit(&state, &user.id, "devops.skill.delete", Some(&id)).await;
    state.service.delete_skill(&user.id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishBatchBody {
    ids: Vec<String>,
    published: bool,
}

async fn publish_skills(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<PublishBatchBody>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    state.service.set_skills_published(&body.ids, body.published).await?;
    Ok(Json(ApiResponse::ok(())))
}

/// Enterprise self-build upload (P1-1 round 1): admin uploads a `SKILL.md`
/// directly instead of typing content/description into the JSON form.
/// Reuses `dream_core_cron::skill_file::validate_skill_content` for the
/// frontmatter/placeholder validation — same parser cron jobs' own skill
/// files go through, not reimplemented here. Fields: `file` (required, the
/// SKILL.md bytes), `categoryId` (optional), `tagIds` (optional, a
/// JSON-encoded array of tag ids — multipart fields are plain text, so a
/// real array field isn't an option here).
async fn upload_skill(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<ApiResponse<SkillRegistryDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;

    let mut file_data: Option<Vec<u8>> = None;
    let mut category_id: Option<String> = None;
    let mut tag_ids: Option<Vec<String>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| DevopsError::BadRequest(format!("multipart error: {e}")))?
    {
        match field.name().unwrap_or("") {
            "file" => {
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| DevopsError::BadRequest(format!("failed to read file: {e}")))?
                        .to_vec(),
                );
            }
            "categoryId" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| DevopsError::BadRequest(format!("failed to read categoryId: {e}")))?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    category_id = Some(trimmed.to_owned());
                }
            }
            "tagIds" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| DevopsError::BadRequest(format!("failed to read tagIds: {e}")))?;
                if !text.trim().is_empty() {
                    tag_ids = Some(
                        serde_json::from_str(&text)
                            .map_err(|_| DevopsError::BadRequest("tagIds must be a JSON array of strings".into()))?,
                    );
                }
            }
            _ => {}
        }
    }

    let file_data = file_data.ok_or_else(|| DevopsError::BadRequest("missing 'file' field".into()))?;
    let content =
        String::from_utf8(file_data).map_err(|_| DevopsError::BadRequest("SKILL.md must be UTF-8 text".into()))?;
    let parsed = dream_core_cron::skill_file::validate_skill_content(&content)
        .map_err(|e| DevopsError::BadRequest(e.to_string()))?;

    let dto = state
        .service
        .upsert_skill(
            None,
            &parsed.name,
            &parsed.description,
            &content,
            true,
            false,
            "org",
            None,
            "all",
            category_id.as_deref(),
            &user.id,
        )
        .await?;
    if let Some(tag_ids) = &tag_ids {
        set_resource_tags_if_wired(&state, "skill", &dto.id, tag_ids).await?;
    }
    audit(&state, &user.id, "devops.skill.upload", Some(&dto.id)).await;
    Ok(Json(ApiResponse::ok(dto)))
}

// -- API assets (P1-6) ------------------------------------------------------

async fn list_api_assets(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<ApiAssetDto>>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(state.service.list_api_assets(&tenant).await?)))
}

/// Import body. `spec` is the raw OpenAPI/Swagger document as a JSON value.
/// YAML is a known limitation: only JSON is accepted (no serde_yaml in the
/// workspace), so the client converts YAML documents before importing.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportApiAssetBody {
    name: String,
    spec: serde_json::Value,
}

async fn import_api_asset(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<ImportApiAssetBody>,
) -> Result<Json<ApiResponse<ApiAssetDto>>, DevopsError> {
    // Same gate as the other registries: an asset is the source for a
    // distributed skill, so in an enterprise only admins import.
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .import_api_asset(&tenant, &user.id, &body.name, &body.spec)
        .await?;
    audit(&state, &user.id, "devops.api_asset.import", Some(&dto.id)).await;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn get_api_asset(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<ApiAssetDetailDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(state.service.get_api_asset(&tenant, &id).await?)))
}

async fn delete_api_asset(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    audit(&state, &user.id, "devops.api_asset.delete", Some(&id)).await;
    state.service.delete_api_asset(&tenant, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

/// Publish body. `baseUrl` overrides the spec-detected base URL in the
/// generated curl examples; `autoActive` marks the published skill
/// admin-required (member agents load it without per-assistant opt-in).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishApiAssetBody {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    auto_active: bool,
}

async fn publish_api_asset(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<PublishApiAssetBody>,
) -> Result<Json<ApiResponse<SkillRegistryDto>>, DevopsError> {
    // Writes into the skill registry are admin-only (see upsert_skill).
    require_registry_admin(&state, &user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .publish_api_asset_skill(&tenant, &user.id, &id, body.base_url.as_deref(), body.auto_active)
        .await?;
    audit(&state, &user.id, "devops.api_asset.publish", Some(&dto.id)).await;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn list_mcp(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<McpRegistryDto>>>, DevopsError> {
    Ok(Json(ApiResponse::ok(state.service.list_mcp_registry(&user.id).await?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertMcpBody {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default = "default_stdio", rename = "type")]
    kind: String,
    #[serde(default)]
    endpoint: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    has_keys: bool,
    /// stdio `env` / sse `headers` JSON, distributed to members (D5).
    #[serde(default)]
    secrets_json: Option<String>,
    /// P0-4 read ACL — see UpsertSkillBody.
    #[serde(default = "default_scope_org")]
    scope: String,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default = "default_visibility_all")]
    visibility: String,
    /// P1-1 round 1. Both optional and orthogonal to scope/visibility.
    #[serde(default)]
    category_id: Option<String>,
    #[serde(default)]
    tag_ids: Option<Vec<String>>,
}

fn default_stdio() -> String {
    "stdio".into()
}

async fn upsert_mcp(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<UpsertMcpBody>,
) -> Result<Json<ApiResponse<McpRegistryDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let dto = state
        .service
        .upsert_mcp_registry(
            body.id.as_deref(),
            &body.name,
            &body.kind,
            &body.endpoint,
            body.enabled,
            body.has_keys,
            body.secrets_json.as_deref(),
            &body.scope,
            body.team_id.as_deref(),
            &body.visibility,
            body.category_id.as_deref(),
            &user.id,
        )
        .await?;
    if let Some(tag_ids) = &body.tag_ids {
        set_resource_tags_if_wired(&state, "mcp", &dto.id, tag_ids).await?;
    }
    audit(&state, &user.id, "devops.mcp.upsert", Some(&dto.id)).await;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_mcp(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    audit(&state, &user.id, "devops.mcp.delete", Some(&id)).await;
    state.service.delete_mcp_registry(&user.id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

async fn publish_mcp(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<PublishBatchBody>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    state.service.set_mcp_published(&body.ids, body.published).await?;
    Ok(Json(ApiResponse::ok(())))
}

// -- company model channels ------------------------------------------------

/// Channels the caller may see. Members get the ones provisioned for them;
/// admins get all of them. Never carries a credential — see `ProviderChannelDto`.
async fn list_model_channels(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<ProviderChannelDto>>>, DevopsError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_provider_channels(&user.id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertModelChannelBody {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default = "default_platform")]
    platform: String,
    upstream_base_url: String,
    /// Write-only. Absent on an edit means "leave the stored credential alone",
    /// which is what lets an admin rename a channel without re-entering it.
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default = "default_models")]
    models: String,
    #[serde(default)]
    model_settings: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_scope_org")]
    scope: String,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default = "default_visibility_all")]
    visibility: String,
}

fn default_platform() -> String {
    "openai".to_owned()
}

fn default_models() -> String {
    "[]".to_owned()
}

async fn upsert_model_channel(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<UpsertModelChannelBody>,
) -> Result<Json<ApiResponse<ProviderChannelDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let dto = state
        .service
        .upsert_provider_channel(
            body.id.as_deref(),
            &body.name,
            &body.platform,
            &body.upstream_base_url,
            body.api_key.as_deref(),
            &body.models,
            body.model_settings.as_deref(),
            body.enabled,
            &body.scope,
            body.team_id.as_deref(),
            &body.visibility,
            &user.id,
        )
        .await?;
    audit(&state, &user.id, "devops.modelChannel.upsert", Some(&dto.id)).await;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_model_channel(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    audit(&state, &user.id, "devops.modelChannel.delete", Some(&id)).await;
    state.service.delete_provider_channel(&user.id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct IssuedChannelTokenDto {
    channel_id: String,
    /// The only time this value is ever transmitted: the server keeps a hash.
    /// The client persists it as the provider's api_key and re-asks only if it
    /// has none.
    token: String,
}

/// Mint this member's token for a channel they can see.
///
/// Not admin-gated on purpose — every member needs their own token, and the
/// authorization is the channel's own visibility (checked in the service).
async fn issue_model_channel_token(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<IssuedChannelTokenDto>>, DevopsError> {
    let issued = state.service.issue_channel_token(&user.id, &id).await?;
    Ok(Json(ApiResponse::ok(IssuedChannelTokenDto {
        channel_id: issued.channel_id,
        token: issued.token,
    })))
}

// -- content inspection (DLP) ---------------------------------------------

/// Every rule, including disabled ones. Admin-only: the full set is the
/// company's control surface.
async fn list_dlp_rules(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<DlpRuleDto>>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    Ok(Json(ApiResponse::ok(state.service.list_dlp_rules().await?)))
}

/// The rules this member is subject to, for local enforcement.
///
/// Deliberately not admin-gated: the check runs on the member's own machine,
/// so their client has to be able to fetch what it is meant to enforce. A rule
/// a member cannot read is a rule that silently does nothing.
async fn list_my_dlp_rules(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<DlpRuleDto>>>, DevopsError> {
    Ok(Json(ApiResponse::ok(
        state.service.list_dlp_rules_for_member(&user.id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertDlpRuleBody {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default = "default_matcher")]
    matcher: String,
    pattern: String,
    #[serde(default = "default_dlp_action")]
    action: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_scope_org")]
    scope: String,
    #[serde(default)]
    team_id: Option<String>,
}

fn default_matcher() -> String {
    "keyword".to_owned()
}

/// New rules record rather than block — see migration 011 for why.
fn default_dlp_action() -> String {
    "log".to_owned()
}

async fn upsert_dlp_rule(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<UpsertDlpRuleBody>,
) -> Result<Json<ApiResponse<DlpRuleDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let dto = state
        .service
        .upsert_dlp_rule(crate::dlp_service::UpsertDlpRule {
            id: body.id.as_deref(),
            name: &body.name,
            matcher: &body.matcher,
            pattern: &body.pattern,
            action: &body.action,
            enabled: body.enabled,
            scope: &body.scope,
            team_id: body.team_id.as_deref(),
            created_by: &user.id,
        })
        .await?;
    audit(&state, &user.id, "devops.dlp.upsert", Some(&dto.id)).await;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_dlp_rule(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    audit(&state, &user.id, "devops.dlp.delete", Some(&id)).await;
    state.service.delete_dlp_rule(&user.id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportDlpEventsBody {
    #[serde(default)]
    events: Vec<DlpEventInput>,
}

/// Accept findings a member's client produced.
///
/// Any member may report their own — the alternative is that findings from
/// non-admins never arrive, which is every finding worth having.
async fn report_dlp_events(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<ReportDlpEventsBody>,
) -> Result<Json<ApiResponse<u64>>, DevopsError> {
    Ok(Json(ApiResponse::ok(
        state.service.record_dlp_events(&user.id, &body.events).await?,
    )))
}

#[derive(Deserialize)]
struct DlpEventsQuery {
    #[serde(default)]
    limit: Option<i64>,
}

async fn list_dlp_events(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Query(query): Query<DlpEventsQuery>,
) -> Result<Json<ApiResponse<Vec<DlpEventDto>>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    Ok(Json(ApiResponse::ok(
        state.service.list_dlp_events(query.limit.unwrap_or(200)).await?,
    )))
}

#[derive(Deserialize)]
struct DlpSummaryQuery {
    /// Inclusive lower bound on `created_at` (ms). Defaults to 0 = all history.
    #[serde(default)]
    since: Option<i64>,
}

/// Aggregated findings (by day / by action) for the reports' security half.
/// Admin-gated exactly like the raw event list — the aggregate narrows the
/// same findings, so it leaks nothing new, but it says nothing a non-admin
/// needs to act on either.
async fn dlp_summary(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Query(query): Query<DlpSummaryQuery>,
) -> Result<Json<ApiResponse<DlpSummaryDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    Ok(Json(ApiResponse::ok(
        state.service.dlp_summary(query.since.unwrap_or(0)).await?,
    )))
}

async fn list_rag(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<RagDocumentDto>>>, DevopsError> {
    Ok(Json(ApiResponse::ok(state.service.list_rag_documents(&user.id).await?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterRagBody {
    title: String,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    file_size: Option<i64>,
    #[serde(default)]
    mime_type: Option<String>,
    /// P0-4 read ACL — see UpsertSkillBody.
    #[serde(default = "default_scope_org")]
    scope: String,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default = "default_visibility_all")]
    visibility: String,
}

async fn register_rag(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<RegisterRagBody>,
) -> Result<Json<ApiResponse<RagDocumentDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let dto = state
        .service
        .register_rag_document(
            &body.title,
            body.file_path.as_deref(),
            body.file_size,
            body.mime_type.as_deref(),
            &body.scope,
            body.team_id.as_deref(),
            &body.visibility,
            &user.id,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_rag(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    state.service.delete_rag_document(&user.id, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(Deserialize)]
struct SetRagContentBody {
    content: String,
}

async fn set_rag_content(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<SetRagContentBody>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    state.service.set_document_content(&user.id, &id, &body.content).await?;
    Ok(Json(ApiResponse::ok(())))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessResult {
    chunk_count: i64,
}

async fn process_rag(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<ProcessResult>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let chunk_count = state.service.process_rag_document(&user.id, &id).await?;
    Ok(Json(ApiResponse::ok(ProcessResult { chunk_count })))
}

async fn get_rag_config(
    State(state): State<OneDevopsRouterState>,
) -> Result<Json<ApiResponse<RagConfigDto>>, DevopsError> {
    Ok(Json(ApiResponse::ok(state.service.get_rag_config().await?)))
}

/// `apiKey` absent = keep stored key; present = replace.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetRagConfigBody {
    base_url: String,
    model: String,
    #[serde(default)]
    api_key: Option<String>,
}

async fn set_rag_config(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<SetRagConfigBody>,
) -> Result<Json<ApiResponse<RagConfigDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let dto = state
        .service
        .set_rag_config(&body.base_url, &body.model, body.api_key.as_deref())
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRagBody {
    query: String,
    #[serde(default)]
    top_k: Option<usize>,
}

async fn search_rag(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<SearchRagBody>,
) -> Result<Json<ApiResponse<Vec<RagSearchHit>>>, DevopsError> {
    let hits = state
        .service
        .search_rag(&user.id, &body.query, body.top_k.unwrap_or(5))
        .await?;
    Ok(Json(ApiResponse::ok(hits)))
}

// -- ownership transfer (P1-2 offboarding) --------------------------------

/// How many team resources a member owns, so the offboarding UI can warn
/// before removal instead of silently orphaning them.
async fn count_owned_resources(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(target_user_id): Path<String>,
) -> Result<Json<ApiResponse<i64>>, DevopsError> {
    state.service.ensure_privileged(&user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    let count = state.service.count_owned_resources(&target_user_id, &tenant).await?;
    Ok(Json(ApiResponse::ok(count)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferOwnershipBody {
    from_user_id: String,
    to_user_id: String,
}

/// Hand a departing member's team resources to another member of the same
/// project group. Admin-only; the tenant is the caller's own, never a
/// client-supplied one, so an admin cannot reach into another group.
async fn transfer_ownership(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<TransferOwnershipBody>,
) -> Result<Json<ApiResponse<i64>>, DevopsError> {
    state.service.ensure_privileged(&user.id).await?;
    let tenant = state.tenant_of(&user.id).await;
    let moved = state
        .service
        .transfer_ownership(&body.from_user_id, &body.to_user_id, &tenant)
        .await?;
    Ok(Json(ApiResponse::ok(moved)))
}

// -- milestones -----------------------------------------------------------

async fn list_milestones(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<MilestoneDto>>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(state.service.list_milestones(&tenant).await?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMilestoneBody {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    due_at: Option<i64>,
}

async fn create_milestone(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateMilestoneBody>,
) -> Result<Json<ApiResponse<MilestoneDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .create_milestone(
            &tenant,
            &user.id,
            Some(user.username.as_str()),
            &body.title,
            body.description.as_deref(),
            body.due_at,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

/// PATCH body: absent field = keep, `null` = clear (for nullable columns).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMilestoneBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default, with = "double_option")]
    description: Option<Option<String>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, with = "double_option")]
    due_at: Option<Option<i64>>,
}

async fn update_milestone(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateMilestoneBody>,
) -> Result<Json<ApiResponse<MilestoneDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .update_milestone(
            &tenant,
            &id,
            body.title.as_deref(),
            body.description.as_ref().map(|d| d.as_deref()),
            body.status.as_deref(),
            body.due_at,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_milestone(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    state.service.delete_milestone(&tenant, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

// -- test plans -----------------------------------------------------------

async fn list_test_plans(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<TestPlanDto>>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(state.service.list_test_plans(&tenant).await?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTestPlanBody {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    requirement_id: Option<String>,
}

async fn create_test_plan(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateTestPlanBody>,
) -> Result<Json<ApiResponse<TestPlanDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .create_test_plan(
            &tenant,
            &user.id,
            Some(user.username.as_str()),
            &body.title,
            body.description.as_deref(),
            body.requirement_id.as_deref(),
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateTestPlanBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default, with = "double_option")]
    description: Option<Option<String>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, with = "double_option")]
    requirement_id: Option<Option<String>>,
}

async fn update_test_plan(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTestPlanBody>,
) -> Result<Json<ApiResponse<TestPlanDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .update_test_plan(
            &tenant,
            &id,
            body.title.as_deref(),
            body.description.as_ref().map(|d| d.as_deref()),
            body.status.as_deref(),
            body.requirement_id.as_ref().map(|r| r.as_deref()),
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_test_plan(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    state.service.delete_test_plan(&tenant, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

// -- test cases -----------------------------------------------------------

async fn list_test_cases(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<TestCaseDto>>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(
        state.service.list_test_cases(&tenant, &id).await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTestCaseBody {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    steps: Option<String>,
    #[serde(default)]
    expected: Option<String>,
}

async fn create_test_case(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(plan_id): Path<String>,
    Json(body): Json<CreateTestCaseBody>,
) -> Result<Json<ApiResponse<TestCaseDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .create_test_case(
            &tenant,
            &plan_id,
            &user.id,
            Some(user.username.as_str()),
            &body.title,
            body.description.as_deref(),
            body.steps.as_deref(),
            body.expected.as_deref(),
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateTestCaseBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, with = "double_option")]
    description: Option<Option<String>>,
    #[serde(default, with = "double_option")]
    steps: Option<Option<String>>,
    #[serde(default, with = "double_option")]
    expected: Option<Option<String>>,
}

async fn update_test_case(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path((_, id)): Path<(String, String)>,
    Json(body): Json<UpdateTestCaseBody>,
) -> Result<Json<ApiResponse<TestCaseDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .update_test_case(
            &tenant,
            &id,
            body.title.as_deref(),
            body.status.as_deref(),
            body.description.as_ref().map(|d| d.as_deref()),
            body.steps.as_ref().map(|s| s.as_deref()),
            body.expected.as_ref().map(|e| e.as_deref()),
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_test_case(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path((_, id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    state.service.delete_test_case(&tenant, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

// -- pipelines ------------------------------------------------------------

async fn list_pipelines(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<PipelineDto>>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(state.service.list_pipelines(&tenant).await?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatePipelineBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    trigger: Option<String>,
}

async fn create_pipeline(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreatePipelineBody>,
) -> Result<Json<ApiResponse<PipelineDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .create_pipeline(
            &tenant,
            &user.id,
            Some(user.username.as_str()),
            &body.name,
            body.description.as_deref(),
            body.trigger.as_deref(),
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdatePipelineBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, with = "double_option")]
    description: Option<Option<String>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    trigger: Option<String>,
}

async fn update_pipeline(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdatePipelineBody>,
) -> Result<Json<ApiResponse<PipelineDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .update_pipeline(
            &tenant,
            &id,
            body.name.as_deref(),
            body.description.as_ref().map(|d| d.as_deref()),
            body.status.as_deref(),
            body.trigger.as_deref(),
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_pipeline(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    state.service.delete_pipeline(&tenant, &id).await?;
    Ok(Json(ApiResponse::ok(())))
}

// -- pipeline runs --------------------------------------------------------

async fn list_pipeline_runs(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<PipelineRunDto>>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    Ok(Json(ApiResponse::ok(
        state.service.list_pipeline_runs(&tenant, &id).await?,
    )))
}

async fn create_pipeline_run(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(pipeline_id): Path<String>,
) -> Result<Json<ApiResponse<PipelineRunDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .create_pipeline_run(&tenant, &pipeline_id, Some(user.username.as_str()))
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdatePipelineRunBody {
    #[serde(default)]
    status: Option<String>,
    #[serde(default, with = "double_option")]
    started_at: Option<Option<i64>>,
    #[serde(default, with = "double_option")]
    finished_at: Option<Option<i64>>,
    #[serde(default, with = "double_option")]
    log: Option<Option<String>>,
}

async fn update_pipeline_run(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path((_, id)): Path<(String, String)>,
    Json(body): Json<UpdatePipelineRunBody>,
) -> Result<Json<ApiResponse<PipelineRunDto>>, DevopsError> {
    let tenant = state.tenant_of(&user.id).await;
    let dto = state
        .service
        .update_pipeline_run(
            &tenant,
            &id,
            body.status.as_deref(),
            body.started_at,
            body.finished_at,
            body.log.as_ref().map(|l| l.as_deref()),
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use dream_core_auth::CurrentUser;
    use sqlx::sqlite::SqlitePoolOptions;
    use tower::ServiceExt;

    use super::*;
    use crate::migrate::run_one_devops_migrations;
    use crate::service::DevopsService;
    use crate::state::OneDevopsRouterState;

    /// In-memory service with the org tables `require_registry_admin` reads.
    /// Same tenant fixture shape as dlp_service's tests: admin1 is org_admin
    /// of tA, memberA an ordinary member of tA.
    async fn router() -> axum::Router {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_one_devops_migrations(&dream_core_db::DbPool::Sqlite(pool.clone())).await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'member', created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, tenant_id));
             CREATE TABLE one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, updated_at INTEGER NOT NULL DEFAULT 0);
             INSERT INTO one_user_org (user_id, tenant_id, role) VALUES ('admin1', 'tA', 'org_admin'), ('memberA', 'tA', 'member');
             INSERT INTO one_active_tenant (user_id, tenant_id) VALUES ('admin1', 'tA'), ('memberA', 'tA');",
        )
        .execute(&pool)
        .await
        .unwrap();
        one_devops_routes(OneDevopsRouterState::new(Arc::new(DevopsService::new(dream_core_db::DbPool::Sqlite(pool.clone())))))
    }

    fn user(id: &str) -> CurrentUser {
        // `local_default` fills the identity fields; only the id matters here.
        let mut u = CurrentUser::local_default();
        u.id = id.to_owned();
        u
    }

    async fn get_summary(router: axum::Router, who: CurrentUser) -> axum::http::Response<Body> {
        let mut request = Request::builder()
            .uri("/api/one/devops/dlp/summary?since=0")
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(who);
        router.oneshot(request).await.unwrap()
    }

    /// The aggregate narrows the same findings the raw event list shows, so it
    /// must sit behind the exact same admin gate — a member reading per-day
    /// findings for the whole company from a "summary" endpoint is the same
    /// leak as reading the raw list, just pre-chewed.
    #[tokio::test]
    async fn dlp_summary_is_admin_only() {
        let router = router().await;

        let response = get_summary(router.clone(), user("memberA")).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = get_summary(router.clone(), user("admin1")).await;
        assert_eq!(response.status(), StatusCode::OK);

        // No org row = standalone/personal mode: the machine owner passes, same
        // as every other registry-write gate in this crate.
        let response = get_summary(router, user("no_org")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Happy path returns the aggregate envelope (empty DB → all zeros), not
    /// just a 200: the status-only assertion above cannot tell a working
    /// endpoint from one that serializes nothing.
    #[tokio::test]
    async fn dlp_summary_returns_the_aggregate_envelope() {
        let router = router().await;
        let response = get_summary(router, user("admin1")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["totalEvents"], 0);
        assert_eq!(json["data"]["totalBlocked"], 0);
    }
}

// --- P1-1 round 2: remote content market ---

async fn list_market_sources(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<MarketSourceDto>>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    Ok(Json(ApiResponse::ok(
        state
            .service
            .list_market_sources(&state.tenant_of(&user.id).await)
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMarketSourceBody {
    name: String,
    /// Absolute http(s) URL of the source's index.json.
    url: String,
}

async fn create_market_source(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<CreateMarketSourceBody>,
) -> Result<Json<ApiResponse<MarketSourceDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let dto = state
        .service
        .create_market_source(&state.tenant_of(&user.id).await, &body.name, &body.url, &user.id)
        .await?;
    audit(&state, &user.id, "devops.market.source.create", Some(&dto.id)).await;
    Ok(Json(ApiResponse::ok(dto)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMarketSourceBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

async fn update_market_source(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateMarketSourceBody>,
) -> Result<Json<ApiResponse<MarketSourceDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let dto = state
        .service
        .update_market_source(
            &state.tenant_of(&user.id).await,
            &id,
            body.name.as_deref(),
            body.url.as_deref(),
            body.enabled,
        )
        .await?;
    Ok(Json(ApiResponse::ok(dto)))
}

async fn delete_market_source(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    state
        .service
        .delete_market_source(&state.tenant_of(&user.id).await, &id)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

/// Synchronous sync: fetch the manifest, pull changed items, return the
/// report. Per-item failures ride in the report; only source-level failures
/// (fetch/manifest) surface as request errors.
async fn sync_market_source(
    State(state): State<OneDevopsRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<MarketSyncReportDto>>, DevopsError> {
    require_registry_admin(&state, &user.id).await?;
    let fetcher = crate::market_sync::ReqwestFetcher::new();
    let tenant = state.tenant_of(&user.id).await;
    // Market sync is a real content-distribution action and can run for
    // minutes over a large manifest — worth an audit row with its wall-clock,
    // and a `failure` row when the manifest fetch / parse fails outright.
    let started = std::time::Instant::now();
    let report = match state.service.sync_market_source(&tenant, &id, &fetcher).await {
        Ok(r) => r,
        Err(e) => {
            state
                .service
                .audit_failure(
                    &tenant,
                    &user.id,
                    "devops.market.sync",
                    Some(&id),
                    Some(started.elapsed().as_millis() as i64),
                )
                .await;
            return Err(e);
        }
    };
    state
        .service
        .audit(
            &tenant,
            &user.id,
            "devops.market.sync",
            Some(&id),
            Some(started.elapsed().as_millis() as i64),
        )
        .await;
    Ok(Json(ApiResponse::ok(report)))
}
