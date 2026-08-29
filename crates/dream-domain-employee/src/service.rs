//! Digital employee orchestration service.
//!
//! Translation of the 1ONE ClaudeCode reference
//! (`src/process/digitalEmployee/DigitalEmployeeRunService.ts` +
//! `TeamDigitalEmployeeRunService.ts`), rebuilt on upstream in-process
//! primitives instead of HTTP-to-self:
//!
//! - personal run → `ConversationService::create` + `run_agent_turn`
//!   (same path the dream-cron `JobExecutor` takes for
//!   `ExecutionMode::NewConversation`)
//! - team run → `TeamSessionService::send_message_to_agent` against the
//!   team's existing slot conversation; completion observed by polling
//!   `get_run_state` until `active_run` becomes `None`
//! - cron → own 30s scanner; scheduling semantics reuses upstream
//!   `dream_core_cron::scheduler::compute_next_run` so At/Every/Cron + tz
//!   behave identically to upstream cron jobs (zero upstream diff)
//! - runHistory JSON blob → `one_employee_runs` table

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use sqlx::SqlitePool;

use dream_core_ai_agent::AgentRegistry;
use dream_core_api_types::{
    AssistantConversationOverridesRequest, AssistantConversationRequest, CreateConversationRequest, CronScheduleDto,
};
use dream_core_common::{AgentType, ProviderWithModel, now_ms};
use dream_core_conversation::{ConversationAgentTurnRequest, ConversationAgentTurnStatus, ConversationService};
use dream_core_cron::scheduler::compute_next_run;
use dream_core_cron::types::schedule_from_dto;
use dream_core_db::{ConversationRowUpdate, IConversationRepository, IProviderRepository};
use dream_core_team::TeamSessionService;

use crate::error::EmployeeError;
use crate::models::{
    CONTENT_RESOURCE_TYPES, ContentCategoryRow, ContentTagRow, EMPLOYEE_GRANT_ALL, EMPLOYEE_GRANT_SUBJECT_TYPES,
    EMPLOYEE_PERMISSION_MANAGE, EMPLOYEE_PERMISSION_USE, EmployeeGrantRow, EmployeeRunRow, PersonalAgentDto,
    PersonalAgentRow, RUN_FAILED, RUN_RUNNING, RUN_SUCCESS, TRIGGER_BREAKDOWN, TRIGGER_CRON, TRIGGER_MANUAL,
};

/// Outcome of a blocking run: the run/conversation linkage plus the agent's
/// full (untruncated) text reply, so callers can parse structured output.
#[derive(Debug, Clone)]
pub struct RunReply {
    pub run_id: String,
    pub conversation_id: String,
    pub reply: String,
}

/// 30s scanner tick — same cadence the TS cron driver used.
const SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Poll `get_run_state` every 3s when waiting for a team run to settle.
const TEAM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
/// Hard ceiling on team-run wait. Mirrors the 15min cap noted in the
/// design doc; prevents a stuck slot from blocking the scanner forever.
const TEAM_POLL_MAX: std::time::Duration = std::time::Duration::from_secs(15 * 60);

pub struct EmployeeService {
    pool: SqlitePool,
    conversation_service: Arc<ConversationService>,
    conversation_repo: Arc<dyn IConversationRepository>,
    agent_registry: Arc<AgentRegistry>,
    team_session_service: Option<Arc<TeamSessionService>>,
    /// Optional so personal-only deployments and unit tests can build the
    /// service without it. When wired, save-time validation additionally
    /// checks that an dream employee's model is offered by an *enabled*
    /// provider; when absent only the shape check runs.
    provider_repo: Option<Arc<dyn IProviderRepository>>,
    work_dir: PathBuf,
}

pub struct CreateEmployeeInput {
    pub name: String,
    pub description: Option<String>,
    pub agent_type: String,
    pub custom_agent_id: Option<String>,
    pub cli_path: Option<String>,
    pub assistant_id: Option<String>,
    pub agent_id_override: Option<String>,
    pub model_id: Option<String>,
    pub model: Option<ProviderWithModel>,
    pub automation_config: Option<serde_json::Value>,
}

/// `None` on any field means "leave unchanged". For the nullable persona/model
/// fields an explicitly-supplied empty string clears the column back to NULL,
/// so a client can detach a persona or a model without deleting the employee.
pub struct UpdateEmployeeInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub agent_type: Option<String>,
    pub assistant_id: Option<String>,
    pub agent_id_override: Option<String>,
    pub model_id: Option<String>,
    pub model: Option<ProviderWithModel>,
    pub automation_config: Option<serde_json::Value>,
}

pub struct ScheduleInput {
    pub schedule: Option<CronScheduleDto>,
    pub enabled: Option<bool>,
}

fn short_id(prefix: &str) -> String {
    let uuid = uuid::Uuid::now_v7().simple().to_string();
    format!("{prefix}_{uuid}")
}

/// `MM/DD HH:mm` (UTC) — same run-conversation naming shape as the TS
/// reference. The name is cosmetic; exact local-time parity is not
/// load-bearing, so we avoid a chrono dependency.
fn format_run_timestamp(now_ms_value: i64) -> String {
    fn leap(y: i64) -> bool {
        (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
    }
    let secs = now_ms_value / 1000;
    let mut rem_days = secs / 86_400;
    let mut year = 1970i64;
    loop {
        let year_days = if leap(year) { 366 } else { 365 };
        if rem_days < year_days {
            break;
        }
        rem_days -= year_days;
        year += 1;
    }
    let month_lengths = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u32;
    for len in month_lengths {
        if rem_days < len {
            break;
        }
        rem_days -= len;
        month += 1;
    }
    let day = rem_days + 1;
    let day_secs = secs % 86_400;
    let (hour, minute) = (day_secs / 3600, (day_secs % 3600) / 60);
    format!("{month:02}/{day:02} {hour:02}:{minute:02}")
}

/// Prompt for a manual/cron run without a bound issue — mirrors the
/// instructions-first fallback chain of `buildPersonalDigitalEmployeeCronPrompt`.
fn build_run_prompt(agent: &PersonalAgentRow) -> String {
    let config: serde_json::Value = serde_json::from_str(&agent.automation_config).unwrap_or_default();
    let instructions = config
        .get("instructions")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(instructions) = instructions {
        return instructions.to_owned();
    }
    if let Some(description) = agent.description.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return format!(
            "你是「{}」。你的职责：{}\n\n请立即执行你的日常职责，完成后输出可交付摘要。",
            agent.name, description
        );
    }
    format!("你是「{}」。请立即执行你的日常职责，完成后输出可交付摘要。", agent.name)
}

/// Employees a user may *use* (own or shared within their tenant). Free
/// function so the sharing predicate can be unit-tested against a bare pool
/// without constructing the full `EmployeeService`.
async fn select_agent_for_use(
    pool: &SqlitePool,
    user_id: &str,
    tenant_id: &str,
    agent_id: &str,
) -> Result<Option<PersonalAgentRow>, sqlx::Error> {
    let Some(row) = sqlx::query_as::<_, PersonalAgentRow>(
        "SELECT * FROM one_personal_agents \
         WHERE id = ? AND (owner_user_id = ? OR (visibility = 'shared' AND tenant_id = ?))",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    if row.owner_user_id == user_id {
        return Ok(Some(row));
    }
    // P1-1 round 1: an unpublished shared employee stays invisible to
    // everyone but its owner, regardless of grants — same convention as
    // skill/mcp's `published` filter. The owner's own access above is
    // unaffected: publish state gates discoverability, not the owner's
    // ability to run/manage their own draft.
    if row.published == 0 {
        return Ok(None);
    }
    match effective_employee_permission(pool, tenant_id, user_id, agent_id).await? {
        Some(_) => Ok(Some(row)),
        None => Ok(None),
    }
}

/// Own employees plus tenant-shared ones the caller has been granted access
/// to. Free function, mirrors `select_agent_for_use` for testability.
async fn select_available_agents(
    pool: &SqlitePool,
    user_id: &str,
    tenant_id: &str,
) -> Result<Vec<PersonalAgentRow>, sqlx::Error> {
    let candidates = sqlx::query_as::<_, PersonalAgentRow>(
        "SELECT * FROM one_personal_agents \
         WHERE owner_user_id = ? OR (visibility = 'shared' AND tenant_id = ?) \
         ORDER BY updated_at DESC",
    )
    .bind(user_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;

    let mut visible = Vec::with_capacity(candidates.len());
    for row in candidates {
        if row.owner_user_id == user_id {
            visible.push(row);
            continue;
        }
        // A non-owner reaching a `shared` row: visible only with an explicit
        // grant now (see migration 006's doc comment on `one_employee_grants`
        // — this tightened `shared`'s default reach on purpose), AND only
        // once published (P1-1 round 1 — an unpublished draft never appears
        // in anyone else's available-agents list, granted or not).
        if row.published != 0
            && effective_employee_permission(pool, tenant_id, user_id, &row.id)
                .await?
                .is_some()
        {
            visible.push(row);
        }
    }
    Ok(visible)
}

/// Ordered so `Manage > Use` — a manager may also run/converse with the
/// employee (`EMPLOYEE_PERMISSION_MANAGE`'s own doc comment: "implied by").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EmployeePermission {
    Use,
    Manage,
}

impl EmployeePermission {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            EMPLOYEE_PERMISSION_USE => Some(Self::Use),
            EMPLOYEE_PERMISSION_MANAGE => Some(Self::Manage),
            _ => None,
        }
    }
}

/// `one_user_org`/`one_departments`/`one_scene_members` belong to
/// `dream-domain-org`/`dream-domain-platform`, which never run their
/// migrations in a personal/standalone deployment (no org at all) — same
/// case `EmployeeService::user_org_role` already handles for `one_user_org`.
/// A missing table here just means "no ancestry/membership to speak of",
/// same as an empty result, not an error.
fn is_missing_table_error(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.message().contains("no such table"))
}

/// Same query shape as `dream_domain_platform::PlatformService::department_ancestry`
/// (walks `one_departments.parent_id` starting from the caller's own
/// department in `one_user_org`), duplicated here rather than depended on —
/// two Domain-layer crates may not depend on each other directly per this
/// repo's architecture rules, and this repo's own established idiom for a
/// lookup this simple is direct SQL against the other crate's table (see
/// `dream_domain_devops::DevopsService::user_org_role`, which does the exact
/// same thing against `one_user_org` for its own admin-role check), not a
/// new trait.
async fn department_ancestry(pool: &SqlitePool, tenant_id: &str, user_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let mut current: Option<String> =
        match sqlx::query_scalar("SELECT department_id FROM one_user_org WHERE tenant_id = ? AND user_id = ?")
            .bind(tenant_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await
        {
            Ok(v) => v.flatten(),
            Err(e) if is_missing_table_error(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };

    let mut chain = Vec::new();
    let mut hops = 0;
    while let Some(department_id) = current {
        if hops >= 64 {
            tracing::warn!(tenant_id, user_id, "department ancestry exceeded 64 hops; truncating");
            break;
        }
        hops += 1;
        chain.push(department_id.clone());
        current = match sqlx::query_scalar("SELECT parent_id FROM one_departments WHERE tenant_id = ? AND id = ?")
            .bind(tenant_id)
            .bind(&department_id)
            .fetch_optional(pool)
            .await
        {
            Ok(v) => v.flatten(),
            Err(e) if is_missing_table_error(&e) => None,
            Err(e) => return Err(e),
        };
    }
    Ok(chain)
}

/// Same query shape as `PlatformService::scene_ids_for_member` — see
/// `department_ancestry`'s doc comment for why this is duplicated SQL, not a
/// shared function, and `is_missing_table_error`'s for the fallback.
async fn scene_ids_for_member(pool: &SqlitePool, tenant_id: &str, user_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let result: Result<Vec<(String,)>, sqlx::Error> =
        sqlx::query_as("SELECT scene_id FROM one_scene_members WHERE tenant_id = ? AND user_id = ?")
            .bind(tenant_id)
            .bind(user_id)
            .fetch_all(pool)
            .await;
    match result {
        Ok(rows) => Ok(rows.into_iter().map(|(id,)| id).collect()),
        Err(e) if is_missing_table_error(&e) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Highest permission a subject list holds on `employee_id` (or the `*`
/// wildcard), for one `subject_type`. `None` when `subject_ids` is empty or
/// no row matches — never an error, same "absence is just absence" contract
/// `fold_grants_for_subjects` uses for the four-type matrix.
async fn max_employee_permission_for_subjects(
    pool: &SqlitePool,
    tenant_id: &str,
    subject_type: &str,
    subject_ids: &[String],
    employee_id: &str,
) -> Result<Option<EmployeePermission>, sqlx::Error> {
    if subject_ids.is_empty() {
        return Ok(None);
    }
    let placeholders = vec!["?"; subject_ids.len()].join(", ");
    let sql = format!(
        "SELECT permission FROM one_employee_grants \
         WHERE tenant_id = ? AND subject_type = ? AND employee_id IN (?, '{EMPLOYEE_GRANT_ALL}') \
         AND subject_id IN ({placeholders})"
    );
    let mut query = sqlx::query_scalar::<_, String>(&sql)
        .bind(tenant_id)
        .bind(subject_type)
        .bind(employee_id);
    for subject_id in subject_ids {
        query = query.bind(subject_id);
    }
    let rows = query.fetch_all(pool).await?;
    Ok(rows.iter().filter_map(|p| EmployeePermission::parse(p)).max())
}

/// Highest permission `user_id` holds on `employee_id` via the
/// resource-authorization matrix — a direct member grant, or one on any
/// ancestor department, or one via scene membership, whichever is highest.
/// Resolves only the non-owner path: the owner is always fully authorized
/// regardless of this function, and callers must check that separately (see
/// `select_agent_for_use`/`get_for_manage`, both of which do).
async fn effective_employee_permission(
    pool: &SqlitePool,
    tenant_id: &str,
    user_id: &str,
    employee_id: &str,
) -> Result<Option<EmployeePermission>, sqlx::Error> {
    let mut best = max_employee_permission_for_subjects(
        pool,
        tenant_id,
        "member",
        std::slice::from_ref(&user_id.to_owned()),
        employee_id,
    )
    .await?;

    let department_ids = department_ancestry(pool, tenant_id, user_id).await?;
    if let Some(p) =
        max_employee_permission_for_subjects(pool, tenant_id, "department", &department_ids, employee_id).await?
    {
        best = Some(best.map_or(p, |b| b.max(p)));
    }

    let scene_ids = scene_ids_for_member(pool, tenant_id, user_id).await?;
    if let Some(p) = max_employee_permission_for_subjects(pool, tenant_id, "scene", &scene_ids, employee_id).await? {
        best = Some(best.map_or(p, |b| b.max(p)));
    }

    Ok(best)
}

/// Upsert one subject's grant on one employee (or `EMPLOYEE_GRANT_ALL`) — same
/// `ON CONFLICT ... DO UPDATE` upsert convention as the other four resource
/// types' matrix.
#[allow(clippy::too_many_arguments)]
async fn grant_employee_access(
    pool: &SqlitePool,
    tenant_id: &str,
    subject_type: &str,
    subject_id: &str,
    employee_id: &str,
    permission: &str,
    granted_by: &str,
) -> Result<(), EmployeeError> {
    if !EMPLOYEE_GRANT_SUBJECT_TYPES.contains(&subject_type) {
        return Err(EmployeeError::BadRequest(format!(
            "unknown subject type '{subject_type}'"
        )));
    }
    if permission != EMPLOYEE_PERMISSION_USE && permission != EMPLOYEE_PERMISSION_MANAGE {
        return Err(EmployeeError::BadRequest(format!(
            "unknown permission '{permission}' (allowed: use/manage)"
        )));
    }
    sqlx::query(
        "INSERT INTO one_employee_grants \
             (id, tenant_id, subject_type, subject_id, employee_id, permission, granted_by, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(tenant_id, subject_type, subject_id, employee_id) \
         DO UPDATE SET permission = excluded.permission, granted_by = excluded.granted_by, \
                        created_at = excluded.created_at",
    )
    .bind(short_id("empgrant"))
    .bind(tenant_id)
    .bind(subject_type)
    .bind(subject_id)
    .bind(employee_id)
    .bind(permission)
    .bind(granted_by)
    .bind(now_ms() as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// Batch set `published` for a tenant's employees (P1-1 round 1). Loops the
/// single-row update rather than one bulk statement, matching this
/// codebase's own batch-operation idiom (`create_breakdown_children` in
/// `dream-domain-devops`) — an id from another tenant is silently a no-op
/// row (`WHERE id = ? AND tenant_id = ?` matches nothing) rather than an
/// error, so one bad id in a batch doesn't fail the whole call.
async fn set_published_batch(
    pool: &SqlitePool,
    tenant_id: &str,
    ids: &[String],
    published: bool,
) -> Result<(), EmployeeError> {
    let now = now_ms() as i64;
    for id in ids {
        sqlx::query("UPDATE one_personal_agents SET published = ?, updated_at = ? WHERE id = ? AND tenant_id = ?")
            .bind(published as i64)
            .bind(now)
            .bind(id)
            .bind(tenant_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

// ── content categories / tags (P1-1 round 1) ────────────────────────────
//
// Shared across skill/mcp/employee (see migration 007's own doc comment for
// why this lives in this crate). `dream-domain-devops` calls these
// functions through `EmployeeService`'s thin wrappers — it already has a
// real Cargo dependency on this crate, so this is a normal function call,
// not a cross-crate SQL read.

fn validate_content_resource_type(resource_type: &str) -> Result<(), EmployeeError> {
    if !CONTENT_RESOURCE_TYPES.contains(&resource_type) {
        return Err(EmployeeError::BadRequest(format!(
            "unknown resource type '{resource_type}' (allowed: skill/mcp/employee)"
        )));
    }
    Ok(())
}

async fn list_categories(
    pool: &SqlitePool,
    tenant_id: &str,
    resource_type: &str,
) -> Result<Vec<ContentCategoryRow>, EmployeeError> {
    validate_content_resource_type(resource_type)?;
    let rows = sqlx::query_as::<_, ContentCategoryRow>(
        "SELECT * FROM one_content_categories WHERE tenant_id = ? AND resource_type = ? \
         ORDER BY sort_order ASC, created_at ASC",
    )
    .bind(tenant_id)
    .bind(resource_type)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn get_category(pool: &SqlitePool, id: &str) -> Result<ContentCategoryRow, EmployeeError> {
    sqlx::query_as::<_, ContentCategoryRow>("SELECT * FROM one_content_categories WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| EmployeeError::BadRequest(format!("category '{id}' not found")))
}

async fn create_category(
    pool: &SqlitePool,
    tenant_id: &str,
    resource_type: &str,
    parent_id: Option<&str>,
    name: &str,
    sort_order: i64,
) -> Result<ContentCategoryRow, EmployeeError> {
    validate_content_resource_type(resource_type)?;
    let name = name.trim();
    if name.is_empty() {
        return Err(EmployeeError::BadRequest("category name must not be empty".into()));
    }
    let id = short_id("cat");
    let now = now_ms() as i64;
    sqlx::query(
        "INSERT INTO one_content_categories \
             (id, tenant_id, parent_id, resource_type, name, sort_order, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(tenant_id)
    .bind(parent_id)
    .bind(resource_type)
    .bind(name)
    .bind(sort_order)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    get_category(pool, &id).await
}

async fn update_category(
    pool: &SqlitePool,
    id: &str,
    parent_id: Option<&str>,
    name: &str,
    sort_order: i64,
) -> Result<ContentCategoryRow, EmployeeError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(EmployeeError::BadRequest("category name must not be empty".into()));
    }
    if parent_id == Some(id) {
        return Err(EmployeeError::BadRequest("a category cannot be its own parent".into()));
    }
    let now = now_ms() as i64;
    let result = sqlx::query(
        "UPDATE one_content_categories SET parent_id = ?, name = ?, sort_order = ?, updated_at = ? WHERE id = ?",
    )
    .bind(parent_id)
    .bind(name)
    .bind(sort_order)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(EmployeeError::BadRequest(format!("category '{id}' not found")));
    }
    get_category(pool, id).await
}

/// Rejects deletion when child categories still point at this one — the
/// tree can't have a dangling parent. Does NOT check whether any
/// skill/mcp/employee row still references this as its `category_id`:
/// those tables live in other crates (skill/mcp in `dream-domain-devops`,
/// which this crate has no read access to — the dependency points the
/// other way), and `category_id` is a soft, unenforced reference by design.
/// A row whose category was deleted just reads as "uncategorized" the next
/// time it's listed, rather than blocking the deletion or requiring a
/// cross-crate integrity sweep.
async fn delete_category(pool: &SqlitePool, id: &str) -> Result<(), EmployeeError> {
    let has_children: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM one_content_categories WHERE parent_id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;
    if has_children {
        return Err(EmployeeError::BadRequest(
            "delete or move this category's sub-categories first".into(),
        ));
    }
    sqlx::query("DELETE FROM one_content_categories WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn list_tags(
    pool: &SqlitePool,
    tenant_id: &str,
    resource_type: &str,
) -> Result<Vec<ContentTagRow>, EmployeeError> {
    validate_content_resource_type(resource_type)?;
    let rows = sqlx::query_as::<_, ContentTagRow>(
        "SELECT * FROM one_content_tags WHERE tenant_id = ? AND resource_type = ? ORDER BY name ASC",
    )
    .bind(tenant_id)
    .bind(resource_type)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn create_tag(
    pool: &SqlitePool,
    tenant_id: &str,
    resource_type: &str,
    name: &str,
) -> Result<ContentTagRow, EmployeeError> {
    validate_content_resource_type(resource_type)?;
    let name = name.trim();
    if name.is_empty() {
        return Err(EmployeeError::BadRequest("tag name must not be empty".into()));
    }
    let existing: Option<ContentTagRow> =
        sqlx::query_as("SELECT * FROM one_content_tags WHERE tenant_id = ? AND resource_type = ? AND name = ?")
            .bind(tenant_id)
            .bind(resource_type)
            .bind(name)
            .fetch_optional(pool)
            .await?;
    if let Some(row) = existing {
        return Ok(row);
    }
    let id = short_id("tag");
    let now = now_ms() as i64;
    sqlx::query("INSERT INTO one_content_tags (id, tenant_id, resource_type, name, created_at) VALUES (?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(tenant_id)
        .bind(resource_type)
        .bind(name)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(ContentTagRow {
        id,
        tenant_id: tenant_id.to_owned(),
        resource_type: resource_type.to_owned(),
        name: name.to_owned(),
        created_at: now,
    })
}

/// Deletes the tag and every link to it — no `ON DELETE CASCADE` is
/// declared (SQLite FKs aren't enforced here), so the link cleanup is
/// explicit.
async fn delete_tag(pool: &SqlitePool, id: &str) -> Result<(), EmployeeError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM one_content_tag_links WHERE tag_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM one_content_tags WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Full replace of one resource's tag set — deletes existing links and
/// inserts the new set in one transaction, matching this crate's own
/// upsert-by-replace convention (see `grant_employee_access`'s sibling
/// functions for the same "caller sends the whole desired state" shape).
async fn set_resource_tags(
    pool: &SqlitePool,
    resource_type: &str,
    resource_id: &str,
    tag_ids: &[String],
) -> Result<(), EmployeeError> {
    validate_content_resource_type(resource_type)?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM one_content_tag_links WHERE resource_type = ? AND resource_id = ?")
        .bind(resource_type)
        .bind(resource_id)
        .execute(&mut *tx)
        .await?;
    for tag_id in tag_ids {
        sqlx::query("INSERT INTO one_content_tag_links (tag_id, resource_type, resource_id) VALUES (?, ?, ?)")
            .bind(tag_id)
            .bind(resource_type)
            .bind(resource_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Tags for one resource — what the tag multi-select shows when editing a
/// single item.
async fn list_resource_tags(
    pool: &SqlitePool,
    resource_type: &str,
    resource_id: &str,
) -> Result<Vec<ContentTagRow>, EmployeeError> {
    let rows = sqlx::query_as::<_, ContentTagRow>(
        "SELECT t.* FROM one_content_tags t \
         JOIN one_content_tag_links l ON l.tag_id = t.id \
         WHERE l.resource_type = ? AND l.resource_id = ? ORDER BY t.name ASC",
    )
    .bind(resource_type)
    .bind(resource_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Tags for a whole page of resources in one query, keyed by `resource_id`
/// — what a list/table view uses to show tag chips per row without an N+1
/// query per item.
async fn list_tags_for_resources(
    pool: &SqlitePool,
    resource_type: &str,
    resource_ids: &[String],
) -> Result<HashMap<String, Vec<ContentTagRow>>, EmployeeError> {
    if resource_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; resource_ids.len()].join(", ");
    let sql = format!(
        "SELECT t.*, l.resource_id AS link_resource_id FROM one_content_tags t \
         JOIN one_content_tag_links l ON l.tag_id = t.id \
         WHERE l.resource_type = ? AND l.resource_id IN ({placeholders})"
    );
    let mut query = sqlx::query(&sql).bind(resource_type);
    for resource_id in resource_ids {
        query = query.bind(resource_id);
    }
    let rows = query.fetch_all(pool).await?;
    let mut by_resource: HashMap<String, Vec<ContentTagRow>> = HashMap::new();
    for row in rows {
        use sqlx::Row;
        let resource_id: String = row.try_get("link_resource_id")?;
        let tag = ContentTagRow {
            id: row.try_get("id")?,
            tenant_id: row.try_get("tenant_id")?,
            resource_type: row.try_get("resource_type")?,
            name: row.try_get("name")?,
            created_at: row.try_get("created_at")?,
        };
        by_resource.entry(resource_id).or_default().push(tag);
    }
    Ok(by_resource)
}

/// Truncate a reply to a 240-char run summary (matches the TS reference).
fn truncate_summary(reply: &str) -> String {
    if reply.chars().count() > 240 {
        let truncated: String = reply.chars().take(237).collect();
        format!("{truncated}…")
    } else {
        reply.to_owned()
    }
}

/// Whether a stored `agent_type` label denotes the dream backend — the only
/// one that takes a top-level conversation model. Compares against the enum's
/// own serde name rather than a literal so a rename can't silently drift.
fn is_aionrs(agent_type: &str) -> bool {
    agent_type.trim() == AgentType::DreamEngine.serde_name()
}

/// Trim an incoming optional string, treating blank as absent. Used so a
/// client sending `""` never writes a whitespace-only id into the DB.
fn normalize_optional(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|s| !s.is_empty()).map(ToOwned::to_owned)
}

/// Update-time merge for a nullable column: field absent → keep `existing`;
/// field present and blank → clear to `None`; otherwise take the new value.
fn merge_optional(incoming: Option<&str>, existing: Option<String>) -> Option<String> {
    match incoming {
        None => existing,
        Some(value) => normalize_optional(Some(value)),
    }
}

fn serialize_model(model: Option<&ProviderWithModel>) -> Result<Option<String>, EmployeeError> {
    model
        .map(|model| {
            serde_json::to_string(model).map_err(|e| EmployeeError::Internal(format!("serialize employee model: {e}")))
        })
        .transpose()
}

/// Only dream conversations carry a meaningful top-level model. Verbatim port
/// of `dream_core_cron::executor::resolve_model` so the employee and cron paths
/// derive the same `(provider_id, model)` — see the divergence warning in
/// `dream_core_conversation::task_options`.
fn resolve_model(agent: &PersonalAgentRow) -> Option<ProviderWithModel> {
    if !is_aionrs(&agent.agent_type) {
        return None;
    }
    agent
        .model
        .as_deref()
        .and_then(|raw| serde_json::from_str::<ProviderWithModel>(raw).ok())
}

/// Build the persona binding for a run. Mirrors
/// `dream_core_cron::executor::build_assistant_request`: `assistant_id` wins, the
/// legacy `custom_agent_id` column is the fallback (which is what finally makes
/// that previously write-only column mean something).
///
/// `conversation_overrides.agent_id` is the *only* channel that can move a
/// persona onto a different backend — once `assistant` is set,
/// `CreateConversationRequest.type` is ignored and the effective agent type
/// comes from the assistant snapshot.
fn build_assistant_request(agent: &PersonalAgentRow) -> Option<AssistantConversationRequest> {
    let assistant_id = normalize_optional(agent.assistant_id.as_deref())
        .or_else(|| normalize_optional(agent.custom_agent_id.as_deref()))?;

    let agent_id = normalize_optional(agent.agent_id_override.as_deref());
    // ACP backends carry the model through the assistant overrides; dream uses
    // the top-level `model` instead (see `resolve_model`).
    let model = if is_aionrs(&agent.agent_type) {
        None
    } else {
        normalize_optional(agent.model_id.as_deref())
    };

    let overrides = (agent_id.is_some() || model.is_some()).then(|| AssistantConversationOverridesRequest {
        agent_id,
        model,
        ..Default::default()
    });

    Some(AssistantConversationRequest {
        id: assistant_id,
        locale: None,
        conversation_overrides: overrides,
    })
}

/// Append an optional task context (e.g. a dispatched requirement) under the
/// employee's base run prompt. Empty/whitespace context is a no-op.
fn append_task_context(mut prompt: String, task_context: Option<&str>) -> String {
    if let Some(context) = task_context.map(str::trim).filter(|s| !s.is_empty()) {
        prompt.push_str("\n\n## 本次任务\n");
        prompt.push_str(context);
    }
    prompt
}

impl EmployeeService {
    pub fn new(
        pool: SqlitePool,
        conversation_service: Arc<ConversationService>,
        conversation_repo: Arc<dyn IConversationRepository>,
        agent_registry: Arc<AgentRegistry>,
        work_dir: PathBuf,
    ) -> Self {
        Self {
            pool,
            conversation_service,
            conversation_repo,
            agent_registry,
            team_session_service: None,
            provider_repo: None,
            work_dir,
        }
    }

    /// Wire the provider repository so `validate_model_binding` can reject an
    /// dream model that no enabled provider offers at save time, instead of
    /// letting the run fail later. Optional, same rationale as
    /// [`Self::with_team_session`].
    pub fn with_provider_repo(mut self, provider_repo: Arc<dyn IProviderRepository>) -> Self {
        self.provider_repo = Some(provider_repo);
        self
    }

    /// Wire the team session service. Optional so personal-only deployments
    /// (and unit tests that don't exercise team paths) can construct an
    /// `EmployeeService` without it. Called by the router builder after the
    /// `TeamRouterState` is built.
    pub fn with_team_session(mut self, team_session_service: Arc<TeamSessionService>) -> Self {
        self.team_session_service = Some(team_session_service);
        self
    }

    fn require_team_session(&self) -> Result<&Arc<TeamSessionService>, EmployeeError> {
        self.team_session_service
            .as_ref()
            .ok_or_else(|| EmployeeError::Internal("team session service not configured".into()))
    }

    /// Reject `(agent_type, model)` combinations the conversation layer would
    /// refuse, at save time rather than at run time.
    ///
    /// Three rules:
    /// 1. An dream employee *needs* a model — without one `provision_run`
    ///    resolves the empty provider sentinel and the run dies with
    ///    `Provider '' not found`, which is the whole bug this binding exists to
    ///    fix. Only enforced when the caller is actually setting the binding
    ///    (`require_model`), so renaming a legacy backend-only employee still
    ///    works.
    /// 2. A top-level model is dream-only — `ConversationService::create`
    ///    returns a hard 400 for any other agent type that carries one.
    /// 3. For dream the model must be offered by an *enabled* provider.
    ///    Same check as `dream_core_team::provisioning::resolve_provider_for_model`;
    ///    skipped when no provider repo is wired.
    async fn validate_model_binding(
        &self,
        owner_user_id: &str,
        agent_type: &str,
        model: Option<&ProviderWithModel>,
        require_model: bool,
    ) -> Result<(), EmployeeError> {
        let Some(model) = model else {
            if require_model && is_aionrs(agent_type) {
                return Err(EmployeeError::BadRequest(
                    "an aionrs employee requires a model; pick one from an enabled provider".into(),
                ));
            }
            return Ok(());
        };

        if !is_aionrs(agent_type) {
            return Err(EmployeeError::BadRequest(format!(
                "a model may only be bound to an aionrs employee; '{agent_type}' resolves its model through its own backend"
            )));
        }
        if model.provider_id.trim().is_empty() || model.model.trim().is_empty() {
            return Err(EmployeeError::BadRequest(
                "model requires both providerId and model".into(),
            ));
        }

        let Some(provider_repo) = self.provider_repo.as_ref() else {
            return Ok(());
        };
        let provider = provider_repo
            .find_by_id(owner_user_id, &model.provider_id)
            .await
            .map_err(|e| EmployeeError::Internal(format!("load provider: {e}")))?
            .ok_or_else(|| EmployeeError::BadRequest(format!("provider '{}' no longer exists", model.provider_id)))?;
        if !provider.enabled {
            return Err(EmployeeError::BadRequest(format!(
                "provider '{}' is disabled; pick a model from an enabled provider",
                model.provider_id
            )));
        }

        Ok(())
    }

    /// Same resolution chain as the cron executor's `parse_agent_type` +
    /// `inject_agent_identity`: native serde names (acp/dream/…) pass
    /// through; backend labels ("claude", "gemini", …) resolve through the
    /// agent registry to `Acp` plus an `agent_id`/`backend` identity in extra.
    async fn resolve_agent_identity(
        &self,
        agent_type_str: &str,
        extra: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<AgentType, EmployeeError> {
        if let Some(meta) = self.agent_registry.find_builtin_by_backend(agent_type_str).await {
            extra.insert("agent_id".into(), serde_json::Value::String(meta.id.clone()));
            if let Some(backend) = meta.backend {
                extra.insert("backend".into(), serde_json::Value::String(backend));
            }
            return Ok(AgentType::Acp);
        }
        serde_json::from_value::<AgentType>(serde_json::Value::String(agent_type_str.to_owned()))
            .map_err(|_| EmployeeError::BadRequest(format!("unknown agent type: {agent_type_str}")))
    }

    // --- CRUD ---

    pub async fn get(&self, owner_user_id: &str, agent_id: &str) -> Result<PersonalAgentRow, EmployeeError> {
        sqlx::query_as::<_, PersonalAgentRow>("SELECT * FROM one_personal_agents WHERE id = ? AND owner_user_id = ?")
            .bind(agent_id)
            .bind(owner_user_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(EmployeeError::NotFound)
    }

    /// Employees the user can pick from: their own, plus employees shared
    /// within their tenant (A1 L3). Personal-tenant users only ever see their
    /// own (they are the sole member of the 'default' tenant).
    pub async fn list_available(&self, user_id: &str, tenant_id: &str) -> Result<Vec<PersonalAgentDto>, EmployeeError> {
        let rows = select_available_agents(&self.pool, user_id, tenant_id).await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Resolve an employee the user is allowed to *use* (dispatch/breakdown):
    /// their own employee, or one shared within their tenant. Ownership for
    /// mutation still goes through `get`. Returns `NotFound` when neither
    /// applies.
    pub async fn resolve_agent_for_use(
        &self,
        user_id: &str,
        tenant_id: &str,
        agent_id: &str,
    ) -> Result<PersonalAgentRow, EmployeeError> {
        select_agent_for_use(&self.pool, user_id, tenant_id, agent_id)
            .await?
            .ok_or(EmployeeError::NotFound)
    }

    /// Unscoped read by id — for re-fetching a row after `get_for_manage`
    /// already proved the caller may act on it. `get`'s own
    /// `WHERE ... AND owner_user_id = ?` would wrongly 404 here for a
    /// non-owner manager.
    async fn get_by_id(&self, agent_id: &str) -> Result<PersonalAgentRow, EmployeeError> {
        sqlx::query_as::<_, PersonalAgentRow>("SELECT * FROM one_personal_agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(EmployeeError::NotFound)
    }

    /// Resolve an employee the caller may *manage* (edit / schedule): their
    /// own, or a `shared` one they hold a `manage` grant on via the
    /// resource-authorization matrix. `NotFound` otherwise — same
    /// don't-distinguish-missing-from-not-yours convention `get` already
    /// uses for a non-owner. Deleting stays owner-only and does not use this
    /// (see `delete`'s doc comment).
    async fn get_for_manage(
        &self,
        user_id: &str,
        tenant_id: &str,
        agent_id: &str,
    ) -> Result<PersonalAgentRow, EmployeeError> {
        if let Ok(row) = self.get(user_id, agent_id).await {
            return Ok(row);
        }
        let row = sqlx::query_as::<_, PersonalAgentRow>(
            "SELECT * FROM one_personal_agents WHERE id = ? AND visibility = 'shared' AND tenant_id = ?",
        )
        .bind(agent_id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(EmployeeError::NotFound)?;
        match effective_employee_permission(&self.pool, tenant_id, user_id, agent_id).await? {
            Some(EmployeePermission::Manage) => Ok(row),
            _ => Err(EmployeeError::NotFound),
        }
    }

    /// Every digital employee in the tenant — the admin registry view. No
    /// owner/visibility filter on purpose: an admin overseeing the tenant
    /// should see everything, including other members' `private` employees,
    /// same as `dream_domain_devops::DevopsService::list_skills`'s
    /// privileged branch does for the other three registries.
    pub async fn list_all_for_tenant(&self, tenant_id: &str) -> Result<Vec<PersonalAgentDto>, EmployeeError> {
        let rows = sqlx::query_as::<_, PersonalAgentRow>(
            "SELECT * FROM one_personal_agents WHERE tenant_id = ? ORDER BY updated_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Batch set `published` for a tenant's employees (P1-1 round 1). Admin
    /// operation — callers must run `require_registry_admin` first, same as
    /// every other `admin/*` route in this crate. Loops the single-row
    /// update rather than one bulk statement, matching this codebase's own
    /// batch-operation idiom (`create_breakdown_children` in
    /// `dream-domain-devops`) — an id from another tenant is silently a
    /// no-op row (`WHERE id = ? AND tenant_id = ?` matches nothing) rather
    /// than an error, so one bad id in a batch doesn't fail the whole call.
    pub async fn set_published_batch(
        &self,
        tenant_id: &str,
        ids: &[String],
        published: bool,
    ) -> Result<(), EmployeeError> {
        set_published_batch(&self.pool, tenant_id, ids, published).await
    }

    /// Same idiom as `dream_domain_devops::DevopsService::user_org_role`:
    /// direct SQL against one-org's table rather than a cross-crate
    /// dependency. `None` when one-org isn't initialized at all (personal /
    /// standalone) — callers treat that as "not an admin gate, let it
    /// through", matching devops's own convention for the same case.
    pub async fn user_org_role(&self, user_id: &str) -> Result<Option<String>, EmployeeError> {
        let result = sqlx::query_scalar::<_, String>(
            "SELECT uo.role FROM one_user_org uo WHERE uo.user_id = ? \
             ORDER BY (uo.tenant_id = (SELECT tenant_id FROM one_active_tenant WHERE user_id = uo.user_id)) DESC, \
                       uo.created_at DESC, uo.tenant_id ASC LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await;
        match result {
            Ok(role) => Ok(role),
            // one-org's tables don't exist at all outside enterprise builds.
            Err(e) if is_missing_table_error(&e) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Grant (or overwrite) one subject's permission on one employee (or
    /// every shared employee, via `EMPLOYEE_GRANT_ALL`). Idempotent —
    /// re-granting the same subject/employee pair updates the permission in
    /// place rather than erroring, matching `one_resource_grants`'s own
    /// upsert convention for the other four resource types.
    pub async fn grant_employee_access(
        &self,
        tenant_id: &str,
        subject_type: &str,
        subject_id: &str,
        employee_id: &str,
        permission: &str,
        granted_by: &str,
    ) -> Result<(), EmployeeError> {
        grant_employee_access(
            &self.pool,
            tenant_id,
            subject_type,
            subject_id,
            employee_id,
            permission,
            granted_by,
        )
        .await
    }

    /// Revoke one subject's grant on one employee. Idempotent — revoking a
    /// grant that doesn't exist is not an error (matches
    /// `PlatformService::revoke_resource`'s own convention).
    pub async fn revoke_employee_access(
        &self,
        tenant_id: &str,
        subject_type: &str,
        subject_id: &str,
        employee_id: &str,
    ) -> Result<(), EmployeeError> {
        sqlx::query(
            "DELETE FROM one_employee_grants \
             WHERE tenant_id = ? AND subject_type = ? AND subject_id = ? AND employee_id = ?",
        )
        .bind(tenant_id)
        .bind(subject_type)
        .bind(subject_id)
        .bind(employee_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Current grants for one subject — what the authorization editor shows
    /// when it's open on that member/department/scene.
    pub async fn list_employee_grants(
        &self,
        tenant_id: &str,
        subject_type: &str,
        subject_id: &str,
    ) -> Result<Vec<EmployeeGrantRow>, EmployeeError> {
        let rows = sqlx::query_as::<_, EmployeeGrantRow>(
            "SELECT * FROM one_employee_grants \
             WHERE tenant_id = ? AND subject_type = ? AND subject_id = ? ORDER BY created_at DESC",
        )
        .bind(tenant_id)
        .bind(subject_type)
        .bind(subject_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ── content categories / tags (P1-1 round 1) ──────────────────────
    //
    // Shared across skill/mcp/employee (see migration 007's own doc comment
    // for why this lives in this crate). `dream-domain-devops` calls these
    // methods directly — it already has a real Cargo dependency on this
    // crate, so this is a normal function call, not a cross-crate SQL read.
    // All thin `&self.pool` wrappers around free functions below, same
    // testability shape as `select_agent_for_use`/`grant_employee_access`.

    pub async fn list_categories(
        &self,
        tenant_id: &str,
        resource_type: &str,
    ) -> Result<Vec<ContentCategoryRow>, EmployeeError> {
        list_categories(&self.pool, tenant_id, resource_type).await
    }

    pub async fn create_category(
        &self,
        tenant_id: &str,
        resource_type: &str,
        parent_id: Option<&str>,
        name: &str,
        sort_order: i64,
    ) -> Result<ContentCategoryRow, EmployeeError> {
        create_category(&self.pool, tenant_id, resource_type, parent_id, name, sort_order).await
    }

    pub async fn update_category(
        &self,
        id: &str,
        parent_id: Option<&str>,
        name: &str,
        sort_order: i64,
    ) -> Result<ContentCategoryRow, EmployeeError> {
        update_category(&self.pool, id, parent_id, name, sort_order).await
    }

    pub async fn delete_category(&self, id: &str) -> Result<(), EmployeeError> {
        delete_category(&self.pool, id).await
    }

    pub async fn list_tags(&self, tenant_id: &str, resource_type: &str) -> Result<Vec<ContentTagRow>, EmployeeError> {
        list_tags(&self.pool, tenant_id, resource_type).await
    }

    pub async fn create_tag(
        &self,
        tenant_id: &str,
        resource_type: &str,
        name: &str,
    ) -> Result<ContentTagRow, EmployeeError> {
        create_tag(&self.pool, tenant_id, resource_type, name).await
    }

    pub async fn delete_tag(&self, id: &str) -> Result<(), EmployeeError> {
        delete_tag(&self.pool, id).await
    }

    pub async fn set_resource_tags(
        &self,
        resource_type: &str,
        resource_id: &str,
        tag_ids: &[String],
    ) -> Result<(), EmployeeError> {
        set_resource_tags(&self.pool, resource_type, resource_id, tag_ids).await
    }

    pub async fn list_resource_tags(
        &self,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<Vec<ContentTagRow>, EmployeeError> {
        list_resource_tags(&self.pool, resource_type, resource_id).await
    }

    pub async fn list_tags_for_resources(
        &self,
        resource_type: &str,
        resource_ids: &[String],
    ) -> Result<HashMap<String, Vec<ContentTagRow>>, EmployeeError> {
        list_tags_for_resources(&self.pool, resource_type, resource_ids).await
    }

    /// Set an employee's visibility ('private' | 'shared'). Owner-only: only
    /// the creator can share or unshare their employee.
    pub async fn set_visibility(
        &self,
        owner_user_id: &str,
        agent_id: &str,
        visibility: &str,
    ) -> Result<PersonalAgentDto, EmployeeError> {
        if visibility != "private" && visibility != "shared" {
            return Err(EmployeeError::BadRequest(format!(
                "invalid visibility: {visibility} (allowed: private/shared)"
            )));
        }
        let result = sqlx::query(
            "UPDATE one_personal_agents SET visibility = ?, updated_at = ? WHERE id = ? AND owner_user_id = ?",
        )
        .bind(visibility)
        .bind(now_ms() as i64)
        .bind(agent_id)
        .bind(owner_user_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(EmployeeError::NotFound);
        }
        Ok(self.get(owner_user_id, agent_id).await?.into())
    }

    pub async fn create(
        &self,
        owner_user_id: &str,
        tenant_id: &str,
        input: CreateEmployeeInput,
    ) -> Result<PersonalAgentDto, EmployeeError> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err(EmployeeError::BadRequest("name is required".into()));
        }
        if input.agent_type.trim().is_empty() {
            return Err(EmployeeError::BadRequest("agentType is required".into()));
        }
        let automation_config = input
            .automation_config
            .unwrap_or_else(|| serde_json::json!({}))
            .to_string();

        let agent_type = input.agent_type.trim();
        self.validate_model_binding(owner_user_id, agent_type, input.model.as_ref(), true)
            .await?;
        let model = serialize_model(input.model.as_ref())?;

        let id = short_id("pa");
        let now = now_ms() as i64;
        sqlx::query(
            "INSERT INTO one_personal_agents \
             (id, owner_user_id, tenant_id, name, description, agent_type, custom_agent_id, cli_path, \
              assistant_id, agent_id_override, model_id, model, \
              automation_config, schedule_enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(&id)
        .bind(owner_user_id)
        .bind(tenant_id)
        .bind(name)
        .bind(&input.description)
        .bind(agent_type)
        .bind(&input.custom_agent_id)
        .bind(&input.cli_path)
        .bind(normalize_optional(input.assistant_id.as_deref()))
        .bind(normalize_optional(input.agent_id_override.as_deref()))
        .bind(normalize_optional(input.model_id.as_deref()))
        .bind(&model)
        .bind(&automation_config)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(self.get(owner_user_id, &id).await?.into())
    }

    pub async fn update(
        &self,
        user_id: &str,
        tenant_id: &str,
        agent_id: &str,
        input: UpdateEmployeeInput,
    ) -> Result<PersonalAgentDto, EmployeeError> {
        let existing = self.get_for_manage(user_id, tenant_id, agent_id).await?;
        let name = match input.name.as_deref().map(str::trim) {
            Some("") => return Err(EmployeeError::BadRequest("name must not be empty".into())),
            Some(name) => name.to_owned(),
            None => existing.name,
        };
        let description = input.description.or(existing.description);
        let automation_config = input
            .automation_config
            .map(|v| v.to_string())
            .unwrap_or(existing.automation_config);

        let agent_type = match input.agent_type.as_deref().map(str::trim) {
            Some("") => return Err(EmployeeError::BadRequest("agentType must not be empty".into())),
            Some(value) => value.to_owned(),
            None => existing.agent_type,
        };
        // Absent field → keep stored value; explicit empty string → clear to NULL.
        let assistant_id = merge_optional(input.assistant_id.as_deref(), existing.assistant_id);
        let agent_id_override = merge_optional(input.agent_id_override.as_deref(), existing.agent_id_override);
        let model_id = merge_optional(input.model_id.as_deref(), existing.model_id);
        let model = match input.model.as_ref() {
            Some(model) => serialize_model(Some(model))?,
            None => existing.model,
        };

        // Re-validate against the *resulting* pair, not the incoming one: changing
        // only the backend on an employee that already stores a model must not
        // leave behind a combination the conversation layer would reject.
        // "dream needs a model" is only enforced when this request actually
        // touches the binding, so renaming a legacy backend-only employee (which
        // predates migration 004 and has no model) is still allowed.
        let effective_model = model
            .as_deref()
            .and_then(|raw| serde_json::from_str::<ProviderWithModel>(raw).ok());
        let touches_binding = input.agent_type.is_some() || input.model.is_some();
        self.validate_model_binding(user_id, &agent_type, effective_model.as_ref(), touches_binding)
            .await?;

        sqlx::query(
            "UPDATE one_personal_agents SET name = ?, description = ?, agent_type = ?, assistant_id = ?, \
             agent_id_override = ?, model_id = ?, model = ?, automation_config = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(&name)
        .bind(&description)
        .bind(&agent_type)
        .bind(&assistant_id)
        .bind(&agent_id_override)
        .bind(&model_id)
        .bind(&model)
        .bind(&automation_config)
        .bind(now_ms() as i64)
        .bind(agent_id)
        .execute(&self.pool)
        .await?;

        // Not `self.get`: `get_for_manage` above already proved `user_id` may
        // act on this row, but a non-owner manager would fail `get`'s own
        // `owner_user_id = ?` filter on the way back out.
        Ok(self.get_by_id(agent_id).await?.into())
    }

    /// Owner-only, deliberately not extended to `manage` grant holders —
    /// deleting someone else's asset is a bigger blast radius than editing
    /// it, so this stays a stricter line than `update`/`set_schedule` for
    /// now (see the `EMPLOYEE_PERMISSION_MANAGE` doc comment).
    pub async fn delete(&self, owner_user_id: &str, agent_id: &str) -> Result<(), EmployeeError> {
        let result = sqlx::query("DELETE FROM one_personal_agents WHERE id = ? AND owner_user_id = ?")
            .bind(agent_id)
            .bind(owner_user_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(EmployeeError::NotFound);
        }
        Ok(())
    }

    // --- schedule ---

    /// Replace the schedule on a personal agent. Recomputes `next_run_at`
    /// against the supplied schedule (or clears it when disabled/removed).
    /// Owner, or a `manage`-grant holder on a `shared` employee.
    pub async fn set_schedule(
        &self,
        user_id: &str,
        tenant_id: &str,
        agent_id: &str,
        input: ScheduleInput,
    ) -> Result<PersonalAgentDto, EmployeeError> {
        // Verify ownership/manage-permission before writing; the row is
        // needed for nothing else here (schedule is overwritten wholesale).
        self.get_for_manage(user_id, tenant_id, agent_id).await?;
        let schedule_json = input
            .schedule
            .as_ref()
            .map(|dto| serde_json::to_value(dto))
            .transpose()
            .map_err(|e| EmployeeError::BadRequest(format!("invalid schedule: {e}")))?
            .map(|v| v.to_string());

        let enabled = input.enabled.unwrap_or_else(|| schedule_json.is_some());
        let next_run_at: Option<i64> = match &input.schedule {
            Some(dto) if enabled => {
                let schedule = schedule_from_dto(dto);
                compute_next_run(&schedule, now_ms()).map(|ts| ts as i64)
            }
            _ => None,
        };

        sqlx::query(
            "UPDATE one_personal_agents \
             SET schedule = ?, schedule_enabled = ?, next_run_at = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(&schedule_json)
        .bind(if enabled { 1 } else { 0 })
        .bind(next_run_at)
        .bind(now_ms() as i64)
        .bind(agent_id)
        .execute(&self.pool)
        .await?;

        Ok(self.get_by_id(agent_id).await?.into())
    }

    // --- runs ---

    pub async fn list_runs(&self, owner_user_id: &str, agent_id: &str) -> Result<Vec<EmployeeRunRow>, EmployeeError> {
        // Ownership check first so foreign agents 404 instead of listing empty.
        self.get(owner_user_id, agent_id).await?;
        let rows = sqlx::query_as::<_, EmployeeRunRow>(
            "SELECT * FROM one_employee_runs WHERE agent_id = ? ORDER BY started_at DESC LIMIT 50",
        )
        .bind(agent_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn get_run(&self, owner_user_id: &str, run_id: &str) -> Result<EmployeeRunRow, EmployeeError> {
        sqlx::query_as::<_, EmployeeRunRow>("SELECT * FROM one_employee_runs WHERE id = ? AND owner_user_id = ?")
            .bind(run_id)
            .bind(owner_user_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(EmployeeError::RunNotFound)
    }

    /// Manual "run now": create a fresh conversation, fire the run prompt as a
    /// hidden turn in the background, record the outcome in
    /// `one_employee_runs`. Returns immediately with `{run_id, conversation_id}`.
    /// Owner, or a same-tenant member the employee is `shared` with and who
    /// holds at least `use` on it via the resource-authorization matrix (see
    /// `resolve_agent_for_use`). Previously owner-only regardless of
    /// `visibility` — sharing an employee alone never actually let anyone
    /// else run it from this route; this closes that gap now that a grant
    /// can say so explicitly.
    pub async fn run_now(
        self: &Arc<Self>,
        user_id: &str,
        tenant_id: &str,
        agent_id: &str,
    ) -> Result<(String, String), EmployeeError> {
        let agent = self.resolve_agent_for_use(user_id, tenant_id, agent_id).await?;
        self.start_personal_run(user_id, &agent, TRIGGER_MANUAL, None).await
    }

    /// Manual run carrying an extra task context (e.g. a devops requirement
    /// dispatched to this employee). The context is appended to the agent's
    /// own run prompt so the turn works the requirement, not just the daily
    /// routine.
    ///
    /// Accepts the caller's tenant so a *shared* employee (A1 L3) can be
    /// driven by any same-tenant member — `resolve_agent_for_use` allows the
    /// owner or a tenant-shared employee. The run itself is owned by the
    /// caller (conversation + workspace + run row), regardless of who owns the
    /// agent definition.
    pub async fn run_now_with_context(
        self: &Arc<Self>,
        user_id: &str,
        tenant_id: &str,
        agent_id: &str,
        task_context: String,
    ) -> Result<(String, String), EmployeeError> {
        let agent = self.resolve_agent_for_use(user_id, tenant_id, agent_id).await?;
        self.start_personal_run(user_id, &agent, TRIGGER_MANUAL, Some(task_context))
            .await
    }

    /// Provision a fresh personal run: create the conversation (with the
    /// agent identity injected into `extra`), ensure a workspace, and insert
    /// the `one_employee_runs` row in `running` state. Returns
    /// `(run_id, conversation_id)`. Shared by the fire-and-forget
    /// (`start_personal_run`) and blocking (`run_prompt_blocking`) paths.
    async fn provision_run(
        &self,
        owner_user_id: &str,
        agent: &PersonalAgentRow,
        trigger_source: &str,
    ) -> Result<(String, String), EmployeeError> {
        let mut extra = serde_json::Map::new();
        extra.insert("one_employee_id".into(), serde_json::Value::String(agent.id.clone()));
        extra.insert(
            "one_employee_owner".into(),
            serde_json::Value::String(agent.owner_user_id.clone()),
        );
        if let Some(cli_path) = agent.cli_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            extra.insert("cli_path".into(), serde_json::Value::String(cli_path.to_owned()));
        }
        let agent_type = self.resolve_agent_identity(&agent.agent_type, &mut extra).await?;

        let now = now_ms() as i64;
        let conversation_name = format!("{} - {}", agent.name, format_run_timestamp(now));
        let assistant = build_assistant_request(agent);
        let req = CreateConversationRequest {
            // With a persona attached the effective agent type is derived from
            // the assistant snapshot (and can be redirected by
            // `conversation_overrides.agent_id`), so an explicit type here would
            // be ignored at best and misleading at worst. Same rule as the cron
            // executor.
            r#type: if assistant.is_some() { None } else { Some(agent_type) },
            name: Some(conversation_name),
            // Was hardcoded `None`, which made every dream employee resolve to
            // the empty provider sentinel and fail with `Provider '' not found`.
            model: resolve_model(agent),
            assistant,
            source: None,
            channel_chat_id: None,
            extra: serde_json::Value::Object(extra),
        };
        let response = self
            .conversation_service
            .create(owner_user_id, req)
            .await
            .map_err(|e| EmployeeError::Internal(format!("create conversation: {e}")))?;
        let conversation_id = response.id.clone();

        self.ensure_workspace(owner_user_id, &conversation_id, &response.extra)
            .await?;

        let run_id = short_id("run");
        sqlx::query(
            "INSERT INTO one_employee_runs \
             (id, agent_id, owner_user_id, tenant_id, conversation_id, status, trigger_source, started_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&run_id)
        .bind(&agent.id)
        .bind(owner_user_id)
        .bind(&agent.tenant_id)
        .bind(&conversation_id)
        .bind(RUN_RUNNING)
        .bind(trigger_source)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok((run_id, conversation_id))
    }

    /// Shared personal-run path for manual and cron triggers. Provisions the
    /// run, then spawns `execute_run` to await the agent turn and persist the
    /// outcome. Returns immediately.
    async fn start_personal_run(
        self: &Arc<Self>,
        owner_user_id: &str,
        agent: &PersonalAgentRow,
        trigger_source: &str,
        task_context: Option<String>,
    ) -> Result<(String, String), EmployeeError> {
        let (run_id, conversation_id) = self.provision_run(owner_user_id, agent, trigger_source).await?;

        let service = Arc::clone(self);
        let owner = owner_user_id.to_owned();
        let run_id_bg = run_id.clone();
        let conversation_id_bg = conversation_id.clone();
        let trigger = trigger_source.to_owned();
        let agent_clone = agent.clone();
        tokio::spawn(async move {
            service
                .execute_run(
                    &owner,
                    &agent_clone,
                    &run_id_bg,
                    &conversation_id_bg,
                    &trigger,
                    task_context,
                )
                .await;
        });

        Ok((run_id, conversation_id))
    }

    /// Blocking run with a fully-supplied prompt: provision the run, await the
    /// agent turn inline (no `build_run_prompt` prepend, no background spawn),
    /// persist the outcome, and return the agent's full text reply so callers
    /// can parse structured output (e.g. devops breakdown → child requirements).
    ///
    /// Accepts the caller's tenant so a shared employee can be used by any
    /// same-tenant member (A1 L3); the run is owned by the caller.
    pub async fn run_prompt_blocking(
        self: &Arc<Self>,
        user_id: &str,
        tenant_id: &str,
        agent_id: &str,
        prompt: String,
    ) -> Result<RunReply, EmployeeError> {
        let agent = self.resolve_agent_for_use(user_id, tenant_id, agent_id).await?;
        let (run_id, conversation_id) = self.provision_run(user_id, &agent, TRIGGER_BREAKDOWN).await?;

        let turn_req = ConversationAgentTurnRequest {
            user_id: user_id.to_owned(),
            conversation_id: conversation_id.clone(),
            content: prompt,
            files: vec![],
            inject_skills: vec![],
            persist_user_message: true,
            user_message_hidden: true,
            on_started: None,
            // Employee/breakdown runs don't force a runtime mode; keep the
            // agent's own resolved mode (matches pre-v0.1.45 behavior).
            required_runtime_mode: None,
        };

        match self.conversation_service.run_agent_turn(turn_req).await {
            Ok(outcome) if outcome.status == ConversationAgentTurnStatus::Completed => {
                let reply = self.extract_latest_reply(&conversation_id).await.unwrap_or_default();
                let summary = truncate_summary(&reply);
                self.persist_run_outcome(&run_id, RUN_SUCCESS, Some(&outcome.turn_id), Some(&summary), None)
                    .await;
                Ok(RunReply {
                    run_id,
                    conversation_id,
                    reply,
                })
            }
            Ok(outcome) => {
                let error = outcome.error_message.unwrap_or_else(|| "agent turn failed".into());
                self.persist_run_outcome(&run_id, RUN_FAILED, Some(&outcome.turn_id), None, Some(&error))
                    .await;
                Err(EmployeeError::Internal(error))
            }
            Err(e) => {
                self.persist_run_outcome(&run_id, RUN_FAILED, None, None, Some(&e.to_string()))
                    .await;
                Err(EmployeeError::Internal(e.to_string()))
            }
        }
    }

    /// Manual "run now" against an existing team slot. Reads the slot's
    /// conversation_id via `TeamSessionService::get_team`, then fires the
    /// run prompt via `send_message_to_agent` (fire-and-ack), then polls
    /// `get_run_state` until the slot settles.
    /// Same access rule as `run_now`: owner, or a same-tenant member holding
    /// at least `use` on a `shared` employee via the matrix. `owner_user_id`
    /// here (kept as-is throughout this function) means the caller driving
    /// the run, not necessarily the employee definition's owner — same
    /// convention `run_now_with_context` already documents for its own
    /// `user_id` parameter.
    pub async fn run_now_team(
        self: &Arc<Self>,
        owner_user_id: &str,
        tenant_id: &str,
        agent_id: &str,
        team_id: &str,
        slot_id: &str,
    ) -> Result<(String, String), EmployeeError> {
        let team_session = self.require_team_session()?.clone();
        let agent = self.resolve_agent_for_use(owner_user_id, tenant_id, agent_id).await?;

        // Resolve the slot's existing conversation_id before we fire — we
        // need it for summary extraction after the run settles.
        let team = team_session
            .get_team(owner_user_id, team_id)
            .await
            .map_err(|e| EmployeeError::Internal(format!("team get_team: {e}")))?;
        let slot = team
            .assistants
            .iter()
            .find(|a| a.slot_id == slot_id)
            .ok_or_else(|| EmployeeError::BadRequest(format!("slot {slot_id} not found in team {team_id}")))?;
        let conversation_id = slot.conversation_id.clone();

        let prompt = build_run_prompt(&agent);
        team_session
            .send_message_to_agent(owner_user_id, team_id, slot_id, &prompt, None)
            .await
            .map_err(|e| EmployeeError::Internal(format!("team send_message_to_agent: {e}")))?;

        let now = now_ms() as i64;
        let run_id = short_id("run");
        sqlx::query(
            "INSERT INTO one_employee_runs \
             (id, agent_id, owner_user_id, tenant_id, team_id, slot_id, conversation_id, status, trigger_source, started_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&run_id)
        .bind(&agent.id)
        .bind(owner_user_id)
        .bind(&agent.tenant_id)
        .bind(team_id)
        .bind(slot_id)
        .bind(&conversation_id)
        .bind(RUN_RUNNING)
        .bind(TRIGGER_MANUAL)
        .bind(now)
        .execute(&self.pool)
        .await?;

        let service = Arc::clone(self);
        let owner = owner_user_id.to_owned();
        let run_id_bg = run_id.clone();
        let team_id_bg = team_id.to_owned();
        let conversation_id_bg = conversation_id.clone();
        tokio::spawn(async move {
            service
                .execute_team_run(&owner, &run_id_bg, &team_id_bg, &conversation_id_bg)
                .await;
        });

        Ok((run_id, conversation_id))
    }

    /// Mirror of the cron executor's fallback: some conversation types come
    /// back without a provisioned workspace; the agent turn needs one.
    async fn ensure_workspace(
        &self,
        owner_user_id: &str,
        conversation_id: &str,
        extra: &serde_json::Value,
    ) -> Result<(), EmployeeError> {
        let workspace = extra
            .get("workspace")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default();
        if !workspace.is_empty() {
            return Ok(());
        }

        let fallback = self
            .work_dir
            .join("conversations")
            .join(format!("one-employee-{conversation_id}"));
        std::fs::create_dir_all(&fallback)
            .map_err(|e| EmployeeError::Internal(format!("create workspace {}: {e}", fallback.display())))?;

        let Some(row) = self.conversation_repo.get(owner_user_id, conversation_id).await? else {
            return Ok(());
        };
        let mut extra_value: serde_json::Value =
            serde_json::from_str(&row.extra).unwrap_or_else(|_| serde_json::json!({}));
        if !extra_value.is_object() {
            extra_value = serde_json::json!({});
        }
        extra_value.as_object_mut().expect("json object").insert(
            "workspace".into(),
            serde_json::Value::String(fallback.to_string_lossy().into_owned()),
        );
        let update = ConversationRowUpdate {
            extra: Some(extra_value.to_string()),
            updated_at: Some(now_ms()),
            ..Default::default()
        };
        self.conversation_repo
            .update(owner_user_id, conversation_id, &update)
            .await?;
        Ok(())
    }

    async fn execute_run(
        &self,
        owner_user_id: &str,
        agent: &PersonalAgentRow,
        run_id: &str,
        conversation_id: &str,
        trigger_source: &str,
        task_context: Option<String>,
    ) {
        let prompt = append_task_context(build_run_prompt(agent), task_context.as_deref());
        let turn_req = ConversationAgentTurnRequest {
            user_id: owner_user_id.to_owned(),
            conversation_id: conversation_id.to_owned(),
            content: prompt,
            files: vec![],
            inject_skills: vec![],
            persist_user_message: true,
            user_message_hidden: true,
            on_started: None,
            // Autopilot employee runs don't force a runtime mode; keep the
            // agent's own resolved mode (matches pre-v0.1.45 behavior).
            required_runtime_mode: None,
        };

        let (status, turn_id, summary, error) = match self.conversation_service.run_agent_turn(turn_req).await {
            Ok(outcome) if outcome.status == ConversationAgentTurnStatus::Completed => {
                let summary = self.extract_summary(conversation_id).await;
                (RUN_SUCCESS, Some(outcome.turn_id), summary, None)
            }
            Ok(outcome) => (
                RUN_FAILED,
                Some(outcome.turn_id),
                None,
                Some(outcome.error_message.unwrap_or_else(|| "agent turn failed".into())),
            ),
            Err(e) => (RUN_FAILED, None, None, Some(e.to_string())),
        };

        self.persist_run_outcome(run_id, status, turn_id.as_deref(), summary.as_deref(), error.as_deref())
            .await;
        if trigger_source == TRIGGER_CRON {
            self.recompute_next_run(&agent.id).await;
        }
    }

    /// Wait for the team slot to settle (`active_run` flips to `None`),
    /// then extract summary from the slot's conversation_id (resolved
    /// up-front in `run_now_team`).
    async fn execute_team_run(&self, owner_user_id: &str, run_id: &str, team_id: &str, conversation_id: &str) {
        let team_session = match self.require_team_session() {
            Ok(svc) => Arc::clone(svc),
            Err(e) => {
                self.persist_run_outcome(run_id, RUN_FAILED, None, None, Some(&e.to_string()))
                    .await;
                return;
            }
        };

        let deadline = std::time::Instant::now() + TEAM_POLL_MAX;
        loop {
            if std::time::Instant::now() >= deadline {
                let msg = "team run poll timed out";
                self.persist_run_outcome(run_id, RUN_FAILED, None, None, Some(msg))
                    .await;
                return;
            }
            let state = match team_session.get_run_state(owner_user_id, team_id).await {
                Ok(s) => s,
                Err(e) => {
                    let msg = format!("get_run_state: {e}");
                    self.persist_run_outcome(run_id, RUN_FAILED, None, None, Some(&msg))
                        .await;
                    return;
                }
            };
            if state.active_run.is_none() {
                break;
            }
            tokio::time::sleep(TEAM_POLL_INTERVAL).await;
        }

        let summary = self.extract_summary(conversation_id).await;
        self.persist_run_outcome(run_id, RUN_SUCCESS, None, summary.as_deref(), None)
            .await;
    }

    async fn persist_run_outcome(
        &self,
        run_id: &str,
        status: &str,
        turn_id: Option<&str>,
        summary: Option<&str>,
        error: Option<&str>,
    ) {
        let result = sqlx::query(
            "UPDATE one_employee_runs SET status = ?, turn_id = ?, summary = ?, error = ?, finished_at = ? \
             WHERE id = ?",
        )
        .bind(status)
        .bind(turn_id)
        .bind(summary)
        .bind(error)
        .bind(now_ms() as i64)
        .bind(run_id)
        .execute(&self.pool)
        .await;
        if let Err(e) = result {
            tracing::error!(run_id, error = %e, "one-employee failed to persist run outcome");
        }
    }

    /// Recompute `next_run_at` for the given agent after a cron-triggered
    /// run lands. Uses upstream `compute_next_run` so semantics match the
    /// cron driver exactly.
    async fn recompute_next_run(&self, agent_id: &str) {
        let row: Result<(Option<String>,), sqlx::Error> =
            sqlx::query_as("SELECT schedule FROM one_personal_agents WHERE id = ?")
                .bind(agent_id)
                .fetch_one(&self.pool)
                .await;
        let Ok((schedule_json,)) = row else { return };
        let Some(schedule_json) = schedule_json else { return };
        let Ok(dto) = serde_json::from_str::<CronScheduleDto>(&schedule_json) else {
            return;
        };
        let schedule = schedule_from_dto(&dto);
        let next = compute_next_run(&schedule, now_ms()).map(|ts| ts as i64);
        let _ = sqlx::query("UPDATE one_personal_agents SET next_run_at = ? WHERE id = ?")
            .bind(next)
            .bind(agent_id)
            .execute(&self.pool)
            .await;
    }

    /// Latest visible assistant text reply, truncated to 240 chars — same
    /// summary rule as the TS reference. Used to fill the run row `summary`.
    async fn extract_summary(&self, conversation_id: &str) -> Option<String> {
        self.extract_latest_reply(conversation_id)
            .await
            .map(|r| truncate_summary(&r))
    }

    /// Latest visible assistant text reply, untruncated. Callers that parse
    /// structured output (breakdown) need the whole thing.
    async fn extract_latest_reply(&self, conversation_id: &str) -> Option<String> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT content FROM messages \
             WHERE conversation_id = ? AND type = 'text' AND position = 'left' \
             ORDER BY created_at DESC LIMIT 12",
        )
        .bind(conversation_id)
        .fetch_all(&self.pool)
        .await
        .ok()?;

        rows.into_iter().find_map(|(content,)| {
            let text = serde_json::from_str::<serde_json::Value>(&content)
                .ok()
                .and_then(|v| {
                    v.get("content")
                        .and_then(|c| c.as_str())
                        .map(str::to_owned)
                        .or_else(|| v.as_str().map(str::to_owned))
                })
                .unwrap_or(content);
            let trimmed = text.trim().to_owned();
            (!trimmed.is_empty()).then_some(trimmed)
        })
    }

    // --- cron scanner ---

    /// Spawn the 30s schedule scanner. Runs for the lifetime of the service.
    /// On each tick: select agents with `schedule_enabled=1 AND
    /// next_run_at <= now`, fire `run_now` with `trigger_source='cron'`,
    /// let `recompute_next_run` reschedule on completion.
    pub fn spawn_scheduler(self: &Arc<Self>) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SCAN_INTERVAL);
            // First tick fires immediately on `tokio::time::Interval` — skip
            // it so we don't fire all due schedules the instant the service
            // starts (prevents a thundering herd during boot).
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = service.scan_once().await {
                    tracing::error!(error = %e, "one-employee cron scanner tick failed");
                }
            }
        });
    }

    /// One scanner pass. Returns the number of agents fired.
    async fn scan_once(self: &Arc<Self>) -> Result<usize, EmployeeError> {
        let now = now_ms() as i64;
        let due: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, owner_user_id FROM one_personal_agents \
             WHERE schedule_enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;

        let mut fired = 0;
        for (agent_id, owner_user_id) in due {
            fired += 1;
            let agent = match self.get(&owner_user_id, &agent_id).await {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(agent_id, error = %e, "one-employee scanner: agent disappeared");
                    continue;
                }
            };

            // Mark as fired immediately by clearing next_run_at — prevents
            // the next tick from re-firing while this run is still in
            // flight. `recompute_next_run` (called at the end of
            // `execute_run`) will set the next fire time.
            let _ = sqlx::query("UPDATE one_personal_agents SET next_run_at = NULL WHERE id = ?")
                .bind(&agent_id)
                .execute(&self.pool)
                .await;

            if let Err(e) = self
                .start_personal_run(&owner_user_id, &agent, TRIGGER_CRON, None)
                .await
            {
                tracing::error!(agent_id, error = %e, "one-employee scanner: start_personal_run failed");
                // Restore next_run_at so we retry on the next tick.
                let _ = self.recompute_next_run(&agent_id).await;
            }
        }
        Ok(fired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate::run_one_employee_migrations;

    #[test]
    fn append_task_context_appends_and_noops() {
        assert_eq!(append_task_context("base".into(), None), "base");
        assert_eq!(append_task_context("base".into(), Some("   ")), "base");
        assert_eq!(
            append_task_context("base".into(), Some("做需求 X")),
            "base\n\n## 本次任务\n做需求 X"
        );
    }

    fn agent_row(agent_type: &str) -> PersonalAgentRow {
        PersonalAgentRow {
            id: "pa_1".into(),
            owner_user_id: "u".into(),
            tenant_id: "default".into(),
            name: "调研员".into(),
            description: Some("每日调研".into()),
            agent_type: agent_type.into(),
            custom_agent_id: None,
            cli_path: None,
            assistant_id: None,
            agent_id_override: None,
            model_id: None,
            model: None,
            automation_config: "{}".into(),
            schedule: None,
            schedule_enabled: 0,
            next_run_at: None,
            visibility: "private".into(),
            origin: "self_built".into(),
            category_id: None,
            published: 1,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn run_prompt_prefers_instructions() {
        let mut agent = agent_row("claude");
        agent.automation_config = r#"{"instructions":"  调研今日热点并输出简报  "}"#.into();
        assert_eq!(build_run_prompt(&agent), "调研今日热点并输出简报");
    }

    #[test]
    fn run_prompt_falls_back_to_description_then_generic() {
        let mut agent = agent_row("claude");
        assert!(build_run_prompt(&agent).contains("每日调研"));

        agent.description = None;
        assert!(build_run_prompt(&agent).contains("日常职责"));
    }

    // ── persona + model binding (migration 004) ────────────────────────

    /// The reported bug: an dream employee used to reach the factory with an
    /// empty provider id and fail with `Provider '' not found`. A bound model
    /// must now survive the round-trip through the stored column.
    #[test]
    fn aionrs_agent_resolves_stored_model() {
        let mut agent = agent_row("dream");
        agent.model = Some(
            serde_json::to_string(&ProviderWithModel {
                provider_id: "prov_1".into(),
                model: "glm-5-2".into(),
                use_model: None,
            })
            .unwrap(),
        );

        let resolved = resolve_model(&agent).expect("aionrs must carry a top-level model");
        assert_eq!(resolved.provider_id, "prov_1");
        assert_eq!(resolved.model, "glm-5-2");
    }

    /// Top-level model is dream-only — `ConversationService::create` returns a
    /// hard 400 otherwise, so ACP employees must never send one even if the
    /// column somehow holds a value.
    #[test]
    fn non_aionrs_agent_never_sends_a_top_level_model() {
        let mut agent = agent_row("claude");
        agent.model = Some(r#"{"provider_id":"prov_1","model":"x","use_model":null}"#.into());
        assert!(resolve_model(&agent).is_none());
    }

    #[test]
    fn unparseable_model_column_degrades_to_none() {
        let mut agent = agent_row("aionrs");
        agent.model = Some("not-json".into());
        assert!(resolve_model(&agent).is_none());
    }

    #[test]
    fn assistant_request_is_absent_without_a_persona() {
        assert!(build_assistant_request(&agent_row("claude")).is_none());
    }

    /// The legacy `custom_agent_id` column was write-only dead data; it now
    /// serves as the fallback persona source, matching the cron executor.
    #[test]
    fn assistant_request_prefers_assistant_id_then_custom_agent_id() {
        let mut agent = agent_row("claude");
        agent.custom_agent_id = Some("legacy_persona".into());
        assert_eq!(build_assistant_request(&agent).unwrap().id, "legacy_persona");

        agent.assistant_id = Some("persona_1".into());
        assert_eq!(build_assistant_request(&agent).unwrap().id, "persona_1");
    }

    /// A manual backend override only takes effect through
    /// `conversation_overrides.agent_id` — once `assistant` is set the
    /// conversation type is derived from the assistant snapshot.
    #[test]
    fn backend_override_travels_as_conversation_override() {
        let mut agent = agent_row("aionrs");
        agent.assistant_id = Some("persona_1".into());
        agent.agent_id_override = Some("agent_meta_9".into());

        let overrides = build_assistant_request(&agent)
            .unwrap()
            .conversation_overrides
            .expect("override must be forwarded");
        assert_eq!(overrides.agent_id.as_deref(), Some("agent_meta_9"));
        // dream takes its model from the top level, not the assistant override.
        assert!(overrides.model.is_none());
    }

    #[test]
    fn acp_persona_carries_model_id_through_the_override() {
        let mut agent = agent_row("claude");
        agent.assistant_id = Some("persona_1".into());
        agent.model_id = Some("claude-sonnet-4-6".into());

        let overrides = build_assistant_request(&agent).unwrap().conversation_overrides.unwrap();
        assert_eq!(overrides.model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn blank_persona_fields_are_treated_as_absent() {
        let mut agent = agent_row("claude");
        agent.assistant_id = Some("   ".into());
        assert!(build_assistant_request(&agent).is_none());

        agent.assistant_id = Some("persona_1".into());
        agent.agent_id_override = Some("  ".into());
        assert!(
            build_assistant_request(&agent)
                .unwrap()
                .conversation_overrides
                .is_none()
        );
    }

    #[test]
    fn merge_optional_keeps_clears_and_replaces() {
        assert_eq!(merge_optional(None, Some("kept".into())), Some("kept".into()));
        assert_eq!(merge_optional(Some(""), Some("kept".into())), None);
        assert_eq!(merge_optional(Some("new"), Some("kept".into())), Some("new".into()));
    }

    async fn insert_agent(pool: &SqlitePool, id: &str, owner: &str, tenant: &str, visibility: &str) {
        sqlx::query(
            "INSERT INTO one_personal_agents \
             (id, owner_user_id, tenant_id, name, agent_type, automation_config, schedule_enabled, visibility, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'claude', '{}', 0, ?, 0, 0)",
        )
        .bind(id)
        .bind(owner)
        .bind(tenant)
        .bind(id)
        .bind(visibility)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_employee_grant(
        pool: &SqlitePool,
        tenant_id: &str,
        subject_type: &str,
        subject_id: &str,
        employee_id: &str,
        permission: &str,
    ) {
        sqlx::query(
            "INSERT INTO one_employee_grants \
             (id, tenant_id, subject_type, subject_id, employee_id, permission, granted_by, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, 'admin1', 0)",
        )
        .bind(uuid::Uuid::now_v7().simple().to_string())
        .bind(tenant_id)
        .bind(subject_type)
        .bind(subject_id)
        .bind(employee_id)
        .bind(permission)
        .execute(pool)
        .await
        .unwrap();
    }

    /// This is the load-bearing semantic change T12's employee-grant work
    /// makes on purpose (align-openocta §3): `shared` alone no longer means
    /// "usable by the whole tenant" the way it did before this feature —
    /// see migration 006's own doc comment. `sharing_requires_an_explicit_grant_now`
    /// below replaces the old `sharing_resolves_own_and_tenant_shared`, which
    /// asserted exactly the opposite (a non-owner seeing a `shared` row with
    /// zero grants configured) — that assertion described the old behavior
    /// correctly, but the old behavior is what this feature exists to change.
    #[tokio::test]
    async fn sharing_requires_an_explicit_grant_now() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();
        // A: a1 private/t1, a2 shared/t1. B: b1 shared/t1. C: c1 shared/t2.
        insert_agent(pool, "a1", "A", "t1", "private").await;
        insert_agent(pool, "a2", "A", "t1", "shared").await;
        insert_agent(pool, "b1", "B", "t1", "shared").await;
        insert_agent(pool, "c1", "C", "t2", "shared").await;

        // Owner always sees their own, regardless of visibility.
        assert!(select_agent_for_use(pool, "A", "t1", "a1").await.unwrap().is_some());
        assert!(select_agent_for_use(pool, "A", "t1", "a2").await.unwrap().is_some());

        // B@t1, with zero grants configured: NOT A's private a1 (never was
        // reachable), and — the new part — NOT A's `shared` a2 either. Own
        // b1 is still visible (ownership, not sharing).
        assert!(select_agent_for_use(pool, "B", "t1", "a1").await.unwrap().is_none());
        assert!(select_agent_for_use(pool, "B", "t1", "a2").await.unwrap().is_none());
        assert!(select_agent_for_use(pool, "B", "t1", "b1").await.unwrap().is_some());

        // Grant B `use` on a2 (a direct member grant) — now B can reach it.
        insert_employee_grant(pool, "t1", "member", "B", "a2", "use").await;
        assert!(select_agent_for_use(pool, "B", "t1", "a2").await.unwrap().is_some());

        // Cross-tenant: A@t2 cannot reach t1-shared b1 even with a grant that
        // would only ever be looked up under tenant t1, but always sees own a1.
        assert!(select_agent_for_use(pool, "A", "t2", "b1").await.unwrap().is_none());
        assert!(select_agent_for_use(pool, "A", "t2", "a1").await.unwrap().is_some());

        // list_available for B@t1 = own b1 + granted a2 (not private a1, not t2 c1).
        let ids: std::collections::HashSet<String> = select_available_agents(pool, "B", "t1")
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, ["a2".to_owned(), "b1".to_owned()].into_iter().collect());
    }

    #[tokio::test]
    async fn wildcard_grant_covers_every_shared_employee() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();
        insert_agent(pool, "a1", "A", "t1", "shared").await;
        insert_agent(pool, "a2", "A", "t1", "shared").await;
        insert_agent(pool, "a3", "A", "t1", "private").await;

        // No grant yet: B sees neither.
        assert!(select_agent_for_use(pool, "B", "t1", "a1").await.unwrap().is_none());

        insert_employee_grant(pool, "t1", "member", "B", EMPLOYEE_GRANT_ALL, "use").await;
        assert!(select_agent_for_use(pool, "B", "t1", "a1").await.unwrap().is_some());
        assert!(select_agent_for_use(pool, "B", "t1", "a2").await.unwrap().is_some());
        // `private` is never in the matrix's reach, wildcard or not.
        assert!(select_agent_for_use(pool, "B", "t1", "a3").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn permission_resolves_via_department_ancestry() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();
        insert_agent(pool, "a1", "A", "t1", "shared").await;

        sqlx::raw_sql(
            "CREATE TABLE one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, \
                 department_id TEXT, PRIMARY KEY (user_id, tenant_id));\
             CREATE TABLE one_departments (id TEXT PRIMARY KEY NOT NULL, tenant_id TEXT NOT NULL, parent_id TEXT);",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO one_user_org (user_id, tenant_id, department_id) VALUES ('B', 't1', 'dept_child')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO one_departments (id, tenant_id, parent_id) VALUES ('dept_child', 't1', 'dept_root')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO one_departments (id, tenant_id, parent_id) VALUES ('dept_root', 't1', NULL)")
            .execute(pool)
            .await
            .unwrap();

        // B is not granted directly, but is in dept_child whose ancestor is
        // dept_root — grant the ancestor, B must still resolve it.
        assert!(select_agent_for_use(pool, "B", "t1", "a1").await.unwrap().is_none());
        insert_employee_grant(pool, "t1", "department", "dept_root", "a1", "use").await;
        assert!(select_agent_for_use(pool, "B", "t1", "a1").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn permission_resolves_via_scene_membership() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();
        insert_agent(pool, "a1", "A", "t1", "shared").await;

        sqlx::raw_sql(
            "CREATE TABLE one_scene_members (scene_id TEXT NOT NULL, tenant_id TEXT NOT NULL, \
                 user_id TEXT NOT NULL, added_at INTEGER NOT NULL, PRIMARY KEY (scene_id, user_id));",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO one_scene_members (scene_id, tenant_id, user_id, added_at) VALUES ('scene_sales', 't1', 'B', 0)")
            .execute(pool)
            .await
            .unwrap();

        assert!(select_agent_for_use(pool, "B", "t1", "a1").await.unwrap().is_none());
        insert_employee_grant(pool, "t1", "scene", "scene_sales", "a1", "use").await;
        assert!(select_agent_for_use(pool, "B", "t1", "a1").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn manage_permission_implies_use_and_outranks_it() {
        assert!(EmployeePermission::Manage > EmployeePermission::Use);

        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();
        insert_agent(pool, "a1", "A", "t1", "shared").await;
        insert_employee_grant(pool, "t1", "member", "B", "a1", "manage").await;

        // A `manage` grant alone is enough to satisfy the (weaker) use check.
        assert!(select_agent_for_use(pool, "B", "t1", "a1").await.unwrap().is_some());
        assert_eq!(
            effective_employee_permission(pool, "t1", "B", "a1").await.unwrap(),
            Some(EmployeePermission::Manage)
        );
    }

    #[tokio::test]
    async fn grant_employee_access_upserts_and_validates_input() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();

        grant_employee_access(pool, "t1", "member", "B", "a1", EMPLOYEE_PERMISSION_USE, "admin1")
            .await
            .unwrap();
        let rows: Vec<EmployeeGrantRow> = sqlx::query_as(
            "SELECT * FROM one_employee_grants WHERE tenant_id = 't1' AND subject_id = 'B' AND employee_id = 'a1'",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].permission, "use");

        // Re-granting the same (tenant, subject_type, subject_id, employee_id)
        // upserts in place rather than duplicating the row.
        grant_employee_access(pool, "t1", "member", "B", "a1", EMPLOYEE_PERMISSION_MANAGE, "admin1")
            .await
            .unwrap();
        let rows: Vec<EmployeeGrantRow> = sqlx::query_as(
            "SELECT * FROM one_employee_grants WHERE tenant_id = 't1' AND subject_id = 'B' AND employee_id = 'a1'",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].permission, "manage");

        assert!(matches!(
            grant_employee_access(pool, "t1", "bogus", "B", "a1", EMPLOYEE_PERMISSION_USE, "admin1").await,
            Err(EmployeeError::BadRequest(_))
        ));
        assert!(matches!(
            grant_employee_access(pool, "t1", "member", "B", "a1", "bogus", "admin1").await,
            Err(EmployeeError::BadRequest(_))
        ));
    }

    #[test]
    fn run_timestamp_shape() {
        // 2026-07-05 01:30 UTC
        let s = format_run_timestamp(1_783_474_200_000);
        assert_eq!(s.len(), 11);
        assert!(s.contains('/') && s.contains(':'));
    }

    #[test]
    fn compute_next_run_every() {
        let dto = CronScheduleDto::Every {
            every_ms: 60_000,
            description: None,
        };
        let schedule = schedule_from_dto(&dto);
        // 1000ms + 60000ms = 61000ms
        assert_eq!(compute_next_run(&schedule, 1000), Some(61_000));
    }

    #[test]
    fn compute_next_run_at_is_absolute() {
        let dto = CronScheduleDto::At {
            at_ms: 5_000,
            description: None,
        };
        let schedule = schedule_from_dto(&dto);
        // At always returns the absolute timestamp regardless of `now`.
        assert_eq!(compute_next_run(&schedule, 1000), Some(5_000));
        assert_eq!(compute_next_run(&schedule, 100_000), Some(5_000));
    }

    // ── content categories / tags / published (P1-1 round 1) ───────────

    async fn insert_agent_with_published(
        pool: &SqlitePool,
        id: &str,
        owner: &str,
        tenant: &str,
        visibility: &str,
        published: i64,
    ) {
        sqlx::query(
            "INSERT INTO one_personal_agents \
             (id, owner_user_id, tenant_id, name, agent_type, automation_config, schedule_enabled, visibility, published, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'claude', '{}', 0, ?, ?, 0, 0)",
        )
        .bind(id)
        .bind(owner)
        .bind(tenant)
        .bind(id)
        .bind(visibility)
        .bind(published)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unpublished_shared_employee_is_invisible_even_with_a_grant() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();
        insert_agent_with_published(pool, "a1", "A", "t1", "shared", 0).await;
        insert_employee_grant(pool, "t1", "member", "B", "a1", "manage").await;

        // Owner still sees their own unpublished draft.
        assert!(select_agent_for_use(pool, "A", "t1", "a1").await.unwrap().is_some());
        // A non-owner with a `manage` grant still can't reach it while unpublished.
        assert!(select_agent_for_use(pool, "B", "t1", "a1").await.unwrap().is_none());
        let ids: std::collections::HashSet<String> = select_available_agents(pool, "B", "t1")
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert!(!ids.contains("a1"));
    }

    #[tokio::test]
    async fn category_tree_crud_and_delete_rejects_with_children() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();

        let root = create_category(pool, "t1", "skill", None, "  运维  ", 0).await.unwrap();
        assert_eq!(root.name, "运维", "name must be trimmed");
        assert!(root.parent_id.is_none());

        let child = create_category(pool, "t1", "skill", Some(&root.id), "Kubernetes", 1)
            .await
            .unwrap();
        assert_eq!(child.parent_id.as_deref(), Some(root.id.as_str()));

        // A category from a different resource_type is invisible to this list.
        create_category(pool, "t1", "mcp", None, "工具分类", 0).await.unwrap();
        let skill_categories = list_categories(pool, "t1", "skill").await.unwrap();
        assert_eq!(skill_categories.len(), 2);

        // Deleting the root while it still has a child is rejected.
        assert!(delete_category(pool, &root.id).await.is_err());

        let updated = update_category(pool, &child.id, None, "K8s", 5).await.unwrap();
        assert_eq!(updated.name, "K8s");
        assert!(updated.parent_id.is_none(), "reassigned to root");

        // Now the (former) root has no children and can be deleted.
        delete_category(pool, &root.id).await.unwrap();
        assert!(
            list_categories(pool, "t1", "skill")
                .await
                .unwrap()
                .iter()
                .any(|c| c.id == child.id)
        );

        assert!(matches!(
            create_category(pool, "t1", "bogus", None, "x", 0).await,
            Err(EmployeeError::BadRequest(_))
        ));
    }

    #[tokio::test]
    async fn tag_create_is_idempotent_by_name_and_delete_cascades_links() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();

        let tag1 = create_tag(pool, "t1", "skill", "生产力").await.unwrap();
        let tag2 = create_tag(pool, "t1", "skill", "生产力").await.unwrap();
        assert_eq!(tag1.id, tag2.id, "re-creating the same name returns the existing row");
        assert_eq!(list_tags(pool, "t1", "skill").await.unwrap().len(), 1);

        set_resource_tags(pool, "skill", "sk_1", std::slice::from_ref(&tag1.id))
            .await
            .unwrap();
        assert_eq!(list_resource_tags(pool, "skill", "sk_1").await.unwrap().len(), 1);

        delete_tag(pool, &tag1.id).await.unwrap();
        assert!(list_resource_tags(pool, "skill", "sk_1").await.unwrap().is_empty());
        assert!(list_tags(pool, "t1", "skill").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_resource_tags_fully_replaces_the_previous_set() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();
        let a = create_tag(pool, "t1", "skill", "a").await.unwrap();
        let b = create_tag(pool, "t1", "skill", "b").await.unwrap();

        set_resource_tags(pool, "skill", "sk_1", &[a.id.clone(), b.id.clone()])
            .await
            .unwrap();
        assert_eq!(list_resource_tags(pool, "skill", "sk_1").await.unwrap().len(), 2);

        set_resource_tags(pool, "skill", "sk_1", std::slice::from_ref(&b.id))
            .await
            .unwrap();
        let remaining = list_resource_tags(pool, "skill", "sk_1").await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, b.id);
    }

    #[tokio::test]
    async fn list_tags_for_resources_batches_without_n_plus_one() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();
        let tag = create_tag(pool, "t1", "skill", "热门").await.unwrap();
        set_resource_tags(pool, "skill", "sk_1", std::slice::from_ref(&tag.id))
            .await
            .unwrap();
        set_resource_tags(pool, "skill", "sk_2", std::slice::from_ref(&tag.id))
            .await
            .unwrap();

        let by_resource = list_tags_for_resources(
            pool,
            "skill",
            &["sk_1".to_owned(), "sk_2".to_owned(), "sk_3".to_owned()],
        )
        .await
        .unwrap();
        assert_eq!(by_resource.get("sk_1").map(Vec::len), Some(1));
        assert_eq!(by_resource.get("sk_2").map(Vec::len), Some(1));
        assert!(!by_resource.contains_key("sk_3"));
    }

    #[tokio::test]
    async fn set_published_batch_is_scoped_to_tenant() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(db.pool()).await.unwrap();
        let pool = db.pool();
        insert_agent(pool, "a1", "A", "t1", "shared").await;
        insert_agent(pool, "a2", "A", "t2", "shared").await;

        set_published_batch(pool, "t1", &["a1".to_owned(), "a2".to_owned()], false)
            .await
            .unwrap();

        let a1_published: i64 = sqlx::query_scalar("SELECT published FROM one_personal_agents WHERE id = 'a1'")
            .fetch_one(pool)
            .await
            .unwrap();
        let a2_published: i64 = sqlx::query_scalar("SELECT published FROM one_personal_agents WHERE id = 'a2'")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(a1_published, 0, "a1 is in tenant t1, must be unpublished");
        assert_eq!(a2_published, 1, "a2 is in tenant t2, the batch call must not touch it");
    }
}
