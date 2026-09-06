//! Team-sync channel for digital employees (P1-3).
//!
//! Enterprise-distributed employees were the one team resource that never
//! reached the desktop client: the member's employee list is read from the
//! LOCAL co-located dreamcore (`personalAgent.list` is hard-wired to the
//! local backend), while distribution decisions live on the enterprise
//! server. Skills/MCP/model-channels each got a local materialization
//! endpoint (`sync_team_servers`, `sync_team_skills`, …); this module is the
//! employee edition, same contract:
//!
//! * The client pulls the team view from the governance plane (the server's
//!   own `GET /api/one/employee/agents` — own + tenant-shared + published +
//!   authorized, already the exact distribution semantics) and pushes it here,
//!   to the LOCAL backend.
//! * `authoritative` MUST be true only when the payload is the complete
//!   current server view (server reachable). Offline passes are
//!   non-authoritative: write-only, so a flaky connection can never wipe the
//!   member's local cache.
//! * Rows this sync owns are marked by [`TEAM_ORIGIN`] in migration 008's
//!   `origin` column — a new value on an existing free-form column, so no
//!   migration — and keep their SERVER id, so reconciliation is a plain
//!   id-set diff and the member's UI needs no mapping.
//! * Reconciliation only ever touches rows carrying the marker: a
//!   self-built, catalog or marketplace row that happens to collide is
//!   reported as a conflict and never rewritten.
//!
//! Deliberately NOT synced: the owner's schedule (scheduling a team employee
//! locally is the member's own choice, and a resync must not silently drop
//! it), and governance columns (visibility/published/category) — the server
//! already decided who may see the agent when it built the payload; locally
//! the row is simply this member's private copy.
//!
//! Model bindings are stored as-is, without `validate_model_binding`: the
//! binding was validated where the admin created the agent, and this machine
//! may legitimately not have that provider yet — the run path treats an
//! unresolvable model exactly like "no model selected".

use std::collections::HashSet;

use dream_core_common::{ProviderWithModel, now_ms};
use dream_core_db::{DbPool, db_params};
use serde::Serialize;

use crate::error::EmployeeError;
use crate::models::PersonalAgentRow;
use crate::service::serialize_model;

/// `origin` value marking a row this sync owns. Distinct from the
/// migration-reserved `'market'`: that one is set aside for a future
/// marketplace semantics, not for "the enterprise distributed this row".
pub const TEAM_ORIGIN: &str = "team";

/// One team-distributed digital employee as the sync client reports it.
/// Mirrors the server-side `PersonalAgentDto` minus the fields that are
/// meaningless locally: governance (`visibility`/`published`/`origin`/
/// `categoryId`), the owner's schedule, and attribution (owner/tenant/
/// timestamps).
#[derive(Debug, Clone)]
pub struct TeamAgentPayload {
    pub id: String,
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

/// Same shape (and same field order) as the MCP sync's report, so the client
/// can treat all four resource syncs uniformly.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamAgentSyncReport {
    pub written: Vec<String>,
    pub removed: Vec<String>,
    pub conflicts: Vec<String>,
    pub kept: usize,
}

/// Materialize the team view into `owner_user_id`'s local registry and, when
/// `authoritative`, reconcile removals. See the module docs for the contract.
pub async fn sync_team_agents(
    pool: &DbPool,
    owner_user_id: &str,
    tenant_id: &str,
    payloads: &[TeamAgentPayload],
    authoritative: bool,
) -> Result<TeamAgentSyncReport, EmployeeError> {
    let mut report = TeamAgentSyncReport::default();
    let mut wanted: HashSet<String> = HashSet::new();

    for payload in payloads {
        if payload.id.trim().is_empty() || payload.name.trim().is_empty() || payload.agent_type.trim().is_empty() {
            return Err(EmployeeError::BadRequest(
                "team-sync payloads need id, name and agentType".into(),
            ));
        }

        // Conflict guard, same shape as `sync_team_servers`: only rows this
        // sync owns (`origin = 'team'`) may be rewritten.
        let existing = pool
            .fetch_optional_as::<PersonalAgentRow>(
                "SELECT * FROM one_personal_agents WHERE id = ?",
                &db_params![&payload.id],
            )
            .await?;
        if let Some(existing) = &existing
            && existing.origin != TEAM_ORIGIN
        {
            report.conflicts.push(payload.name.clone());
            continue;
        }

        let automation_config = payload
            .automation_config
            .clone()
            .unwrap_or_else(|| serde_json::json!({}))
            .to_string();
        let model = serialize_model(payload.model.as_ref())?;
        let now = now_ms() as i64;

        if existing.is_some() {
            // Schedule columns are intentionally untouched on update — a
            // schedule the member set locally is their choice, not the
            // server's to clear (see the module docs).
            pool.execute(
                "UPDATE one_personal_agents SET owner_user_id = ?, tenant_id = ?, name = ?, description = ?, \
                 agent_type = ?, custom_agent_id = ?, cli_path = ?, assistant_id = ?, agent_id_override = ?, \
                 model_id = ?, model = ?, automation_config = ?, visibility = 'private', origin = 'team', \
                 published = 1, updated_at = ? \
                 WHERE id = ?",
                &db_params![
                    owner_user_id,
                    tenant_id,
                    &payload.name,
                    &payload.description,
                    &payload.agent_type,
                    &payload.custom_agent_id,
                    &payload.cli_path,
                    &payload.assistant_id,
                    &payload.agent_id_override,
                    &payload.model_id,
                    &model,
                    &automation_config,
                    now,
                    &payload.id
                ],
            )
            .await?;
        } else {
            pool.execute(
                "INSERT INTO one_personal_agents \
                 (id, owner_user_id, tenant_id, name, description, agent_type, custom_agent_id, cli_path, \
                  assistant_id, agent_id_override, model_id, model, automation_config, schedule_enabled, \
                  visibility, origin, published, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 'private', 'team', 1, ?, ?)",
                &db_params![
                    &payload.id,
                    owner_user_id,
                    tenant_id,
                    &payload.name,
                    &payload.description,
                    &payload.agent_type,
                    &payload.custom_agent_id,
                    &payload.cli_path,
                    &payload.assistant_id,
                    &payload.agent_id_override,
                    &payload.model_id,
                    &model,
                    &automation_config,
                    now,
                    now
                ],
            )
            .await?;
        }
        wanted.insert(payload.id.clone());
        report.written.push(payload.name.clone());
    }

    if authoritative {
        let team_rows = pool
            .fetch_all_as::<PersonalAgentRow>(
                "SELECT * FROM one_personal_agents WHERE owner_user_id = ? AND origin = 'team'",
                &db_params![owner_user_id],
            )
            .await?;
        for row in team_rows {
            if !wanted.contains(&row.id) {
                pool.execute(
                    "DELETE FROM one_personal_agents WHERE id = ? AND owner_user_id = ?",
                    &db_params![&row.id, owner_user_id],
                )
                .await?;
                report.removed.push(row.name);
            }
        }
    }

    report.kept = wanted.len();
    report.written.sort();
    report.removed.sort();
    report.conflicts.sort();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> DbPool {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let pool = DbPool::Sqlite(db.pool().clone());
        crate::migrate::run_one_employee_migrations(&pool).await.unwrap();
        pool
    }

    fn payload(id: &str, name: &str) -> TeamAgentPayload {
        TeamAgentPayload {
            id: id.into(),
            name: name.into(),
            description: Some(format!("{name} description")),
            agent_type: "dream".into(),
            custom_agent_id: None,
            cli_path: None,
            assistant_id: None,
            agent_id_override: None,
            model_id: None,
            model: None,
            automation_config: Some(serde_json::json!({ "instructions": "do the thing" })),
        }
    }

    async fn get_row(pool: &DbPool, id: &str) -> Option<PersonalAgentRow> {
        pool.fetch_optional_as::<PersonalAgentRow>("SELECT * FROM one_personal_agents WHERE id = ?", &db_params![id])
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn authoritative_sync_writes_rewrites_and_reconciles_removals() {
        let pool = test_pool().await;
        let payloads = [payload("pa-srv-1", "Ops"), payload("pa-srv-2", "HR")];

        let report = sync_team_agents(&pool, "member-1", "t1", &payloads, true)
            .await
            .unwrap();
        assert_eq!(report.written, ["HR", "Ops"]);
        assert!(report.removed.is_empty() && report.conflicts.is_empty());
        assert_eq!(report.kept, 2);

        let row = get_row(&pool, "pa-srv-1").await.unwrap();
        assert_eq!(row.owner_user_id, "member-1");
        assert_eq!(row.tenant_id, "t1");
        assert_eq!(row.origin, "team");
        assert_eq!(row.visibility, "private");
        assert_eq!(row.published, 1);
        assert!(row.automation_config.contains("do the thing"));

        // The server id is kept verbatim — that is the reconciliation key.
        assert_eq!(row.id, "pa-srv-1");

        // Second, authoritative pass without pa-srv-2: it is removed, and
        // pa-srv-1 is updated in place rather than duplicated.
        let report = sync_team_agents(&pool, "member-1", "t1", &[payload("pa-srv-1", "Ops v2")], true)
            .await
            .unwrap();
        assert_eq!(report.written, ["Ops v2"]);
        assert_eq!(report.removed, ["HR"]);
        assert_eq!(report.kept, 1);
        assert!(get_row(&pool, "pa-srv-2").await.is_none());
        let row = get_row(&pool, "pa-srv-1").await.unwrap();
        assert_eq!(row.name, "Ops v2");
        let count: i64 = pool
            .fetch_one_scalar("SELECT COUNT(*) FROM one_personal_agents WHERE id = 'pa-srv-1'", &[])
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn non_authoritative_sync_writes_but_never_removes() {
        let pool = test_pool().await;
        sync_team_agents(
            &pool,
            "member-1",
            "t1",
            &[payload("pa-1", "A"), payload("pa-2", "B")],
            true,
        )
        .await
        .unwrap();

        // An offline-ish pass with an empty view must not wipe the cache.
        let report = sync_team_agents(&pool, "member-1", "t1", &[], false).await.unwrap();
        assert!(report.written.is_empty() && report.removed.is_empty());
        assert!(get_row(&pool, "pa-1").await.is_some());
        assert!(get_row(&pool, "pa-2").await.is_some());
    }

    #[tokio::test]
    async fn sync_never_clobbers_a_non_team_row() {
        let pool = test_pool().await;
        pool.execute(
            "INSERT INTO one_personal_agents \
             (id, owner_user_id, tenant_id, name, agent_type, automation_config, origin, created_at, updated_at) \
             VALUES ('pa-local', 'member-1', 't1', 'My Own', 'dream', '{}', 'self_built', 1, 1)",
            &[],
        )
        .await
        .unwrap();

        let report = sync_team_agents(&pool, "member-1", "t1", &[payload("pa-local", "Imposter")], true)
            .await
            .unwrap();
        assert_eq!(report.conflicts, ["Imposter"]);
        assert!(report.written.is_empty() && report.kept == 0);

        let row = get_row(&pool, "pa-local").await.unwrap();
        assert_eq!(row.name, "My Own", "the member's own row is untouched");
        assert_eq!(row.origin, "self_built");
    }

    #[tokio::test]
    async fn a_local_schedule_on_a_team_row_survives_resync() {
        let pool = test_pool().await;
        sync_team_agents(&pool, "member-1", "t1", &[payload("pa-1", "A")], true)
            .await
            .unwrap();
        pool.execute(
            "UPDATE one_personal_agents SET schedule = '{\"kind\":\"cron\"}', schedule_enabled = 1, \
             next_run_at = 12345 WHERE id = 'pa-1'",
            &[],
        )
        .await
        .unwrap();

        sync_team_agents(&pool, "member-1", "t1", &[payload("pa-1", "A renamed")], true)
            .await
            .unwrap();

        let row = get_row(&pool, "pa-1").await.unwrap();
        assert_eq!(row.name, "A renamed", "server-owned fields do update");
        assert!(
            row.schedule.is_some(),
            "a locally-set schedule is not the server's to clear"
        );
        assert_eq!(row.schedule_enabled, 1);
        assert_eq!(row.next_run_at, Some(12345));
    }

    #[tokio::test]
    async fn malformed_payloads_are_rejected_without_partial_writes() {
        let pool = test_pool().await;
        let mut bad = payload("pa-ok", "Fine");
        bad.name = "  ".into();
        let error = sync_team_agents(&pool, "member-1", "t1", &[bad], false)
            .await
            .unwrap_err();
        assert!(matches!(error, EmployeeError::BadRequest(_)));
    }
}
