//! Approval workflow service (P2-1). All logic lives here; routes only
//! transform request/response. The blocking wait used by the terminal-tool
//! approval hot path polls the ledger — two binaries share one SQLite pool,
//! so a decision made on the admin process is seen by the wait on the main
//! process on its next poll (T2 verified shared-DB concurrent access).

use sqlx::SqlitePool;

use dream_core_common::{generate_prefixed_id, now_ms};

use crate::error::WorkflowError;
use crate::models::WorkflowTaskDto;

/// The five OpenOcta-aligned approval classes (§3 已定口径): 创作 / 资源 /
/// 安全策略模板申请 / 工具 / Prompt. The service treats them as a validated
/// vocabulary, nothing more — kind-specific meaning lives with the submitter.
pub const WORKFLOW_TASK_KINDS: [&str; 5] = ["creation", "resource", "security_policy_template", "tool", "prompt"];

/// Terminal-tool approvals block the agent's tool call until this deadline;
/// on expiry the call is **denied** — the conservative default (§3: OpenOcta
/// does not expose its own value). Ten minutes: long enough for an admin to
/// actually respond, short enough that an unattended agent does not hang a
/// whole work session on an approval nobody is coming to click.
pub const TERMINAL_APPROVAL_TIMEOUT_MS: i64 = 10 * 60 * 1000;

/// How often the blocking wait re-reads the task's status.
pub const APPROVAL_POLL_INTERVAL_MS: u64 = 1000;

/// The caller's resolved enterprise membership (active tenant + role).
#[derive(Debug, Clone)]
pub struct WorkflowActor {
    pub tenant_id: String,
    pub role: String,
}

pub struct WorkflowService {
    pool: SqlitePool,
}

fn is_admin_role(role: &str) -> bool {
    matches!(role, "org_admin" | "system_admin" | "admin")
}

/// What [`WorkflowService::wait_for_decision`] reports to the hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// A tenant admin approved — the tool call may proceed.
    Approved,
    /// Rejected by an admin, or the deadline passed: either way the call is
    /// denied and the string explains which.
    Denied { reason: String },
}

type TaskRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    i64,
);

fn row_to_dto(row: TaskRow) -> WorkflowTaskDto {
    let (id, kind, title, detail, payload, requester_id, status, decided_by, decided_at, note, expires_at, created_at) =
        row;
    WorkflowTaskDto {
        id,
        kind,
        title,
        detail,
        payload: serde_json::from_str(&payload).unwrap_or_default(),
        requester_id,
        status,
        decided_by,
        decided_at,
        note,
        expires_at,
        created_at,
    }
}

impl WorkflowService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Resolve the caller's active-tenant membership (same cross-crate query
    /// as one-platform's `resolve_actor`).
    pub async fn resolve_actor(&self, user_id: &str) -> Result<Option<WorkflowActor>, WorkflowError> {
        let result = sqlx::query_as::<_, (String, String)>(
            "SELECT uo.tenant_id, uo.role FROM one_user_org uo WHERE uo.user_id = ? \
             ORDER BY (uo.tenant_id = (SELECT tenant_id FROM one_active_tenant WHERE user_id = uo.user_id)) DESC, \
                      uo.created_at DESC, uo.tenant_id ASC LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await;
        match result {
            Ok(Some((tenant_id, role))) => Ok(Some(WorkflowActor { tenant_id, role })),
            Ok(None) => Ok(None),
            Err(sqlx::Error::Database(e)) if e.message().contains("no such table") => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn require_admin(&self, user_id: &str) -> Result<WorkflowActor, WorkflowError> {
        match self.resolve_actor(user_id).await? {
            None => Err(WorkflowError::NotInEnterprise),
            Some(actor) if !is_admin_role(&actor.role) => {
                Err(WorkflowError::Forbidden("Administrator role required".into()))
            }
            Some(actor) => Ok(actor),
        }
    }

    pub async fn require_member(&self, user_id: &str) -> Result<WorkflowActor, WorkflowError> {
        match self.resolve_actor(user_id).await? {
            None => Err(WorkflowError::NotInEnterprise),
            Some(actor) => Ok(actor),
        }
    }

    /// Create one approval task. `expires_at` is only meaningful for the
    /// blocking terminal-tool flow; member-submitted tasks have no deadline —
    /// they wait for a human as long as it takes.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_task(
        &self,
        tenant_id: &str,
        kind: &str,
        requester_id: &str,
        title: &str,
        detail: &str,
        payload: &serde_json::Value,
        expires_at: Option<i64>,
    ) -> Result<WorkflowTaskDto, WorkflowError> {
        if !WORKFLOW_TASK_KINDS.contains(&kind) {
            return Err(WorkflowError::BadRequest(format!(
                "unknown workflow task kind '{kind}' (expected one of {WORKFLOW_TASK_KINDS:?})"
            )));
        }
        let title = title.trim();
        if title.is_empty() {
            return Err(WorkflowError::BadRequest("task title must not be empty".into()));
        }
        let id = generate_prefixed_id("wft");
        let now = now_ms();
        sqlx::query(
            "INSERT INTO one_workflow_tasks \
                 (id, tenant_id, kind, title, detail, payload, requester_id, status, created_at, expires_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
        )
        .bind(&id)
        .bind(tenant_id)
        .bind(kind)
        .bind(title)
        .bind(detail.trim())
        .bind(payload.to_string())
        .bind(requester_id)
        .bind(now)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        self.get_task(tenant_id, &id)
            .await?
            .ok_or_else(|| WorkflowError::Internal("workflow task vanished immediately after insert".into()))
    }

    /// One task, with lazy expiry applied first — a read must never report a
    /// deadline-passed task as still `pending`.
    pub async fn get_task(&self, tenant_id: &str, id: &str) -> Result<Option<WorkflowTaskDto>, WorkflowError> {
        self.expire_stale().await?;
        let row: Option<TaskRow> = sqlx::query_as(
            "SELECT id, kind, title, detail, payload, requester_id, status, decided_by, decided_at, note, expires_at, created_at \
             FROM one_workflow_tasks WHERE tenant_id = ? AND id = ?",
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(row_to_dto))
    }

    /// Mark every deadline-passed pending task as expired, across tenants.
    /// Lazy by design — the terminal-tool wait loop and every list read keep
    /// the status truthful without a scheduler process.
    async fn expire_stale(&self) -> Result<(), WorkflowError> {
        sqlx::query(
            "UPDATE one_workflow_tasks SET status = 'expired' \
             WHERE status = 'pending' AND expires_at IS NOT NULL AND expires_at < ?",
        )
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// One approval view. `view`:
    /// - `"pending"` — the admin queue (待办)
    /// - `"decided"` — approved / rejected / expired (已办)
    /// - `"mine"` — everything the calling member submitted
    pub async fn list_tasks(
        &self,
        tenant_id: &str,
        view: &str,
        requester_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<WorkflowTaskDto>, WorkflowError> {
        self.expire_stale().await?;
        let status_filter = match view {
            "pending" => "status = 'pending'",
            "decided" => "status IN ('approved', 'rejected', 'expired')",
            "mine" => {
                let Some(_) = requester_id else {
                    return Err(WorkflowError::BadRequest("the 'mine' view needs a requester id".into()));
                };
                "requester_id = ?"
            }
            other => return Err(WorkflowError::BadRequest(format!("unknown task view '{other}'"))),
        };
        let sql = format!(
            "SELECT id, kind, title, detail, payload, requester_id, status, decided_by, decided_at, note, expires_at, created_at \
             FROM one_workflow_tasks WHERE tenant_id = ? AND {status_filter} \
             ORDER BY created_at DESC LIMIT ?"
        );
        let mut query = sqlx::query_as::<_, TaskRow>(&sql).bind(tenant_id);
        if view == "mine" {
            query = query.bind(requester_id.unwrap_or_default());
        }
        let rows = query.bind(limit).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(row_to_dto).collect())
    }

    /// Approve or reject a pending task. Terminal to the task: only a
    /// `pending` row can be decided, and an expired one is already a denial
    /// the requester's agent has acted on — re-deciding it would rewrite
    /// history the hot path already saw.
    pub async fn decide(
        &self,
        tenant_id: &str,
        id: &str,
        decision: &str,
        decided_by: &str,
        note: Option<&str>,
    ) -> Result<WorkflowTaskDto, WorkflowError> {
        let status = match decision {
            "approved" => "approved",
            "rejected" => "rejected",
            other => return Err(WorkflowError::BadRequest(format!("unknown decision '{other}'"))),
        };
        self.expire_stale().await?;
        let now = now_ms();
        let result = sqlx::query(
            "UPDATE one_workflow_tasks SET status = ?, decided_by = ?, decided_at = ?, note = ? \
             WHERE tenant_id = ? AND id = ? AND status = 'pending'",
        )
        .bind(status)
        .bind(decided_by)
        .bind(now)
        .bind(note.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()))
        .bind(tenant_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            let exists: Option<(String,)> =
                sqlx::query_as("SELECT id FROM one_workflow_tasks WHERE tenant_id = ? AND id = ?")
                    .bind(tenant_id)
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?;
            return match exists {
                None => Err(WorkflowError::NotFound("workflow task not found".into())),
                Some(_) => Err(WorkflowError::BadRequest(
                    "task is no longer pending — it was already decided or has expired".into(),
                )),
            };
        }
        self.get_task(tenant_id, id)
            .await?
            .ok_or_else(|| WorkflowError::Internal("workflow task vanished immediately after decision".into()))
    }

    /// Block until the task is decided or `timeout_ms` passes. Polls the
    /// ledger (see the module docs for why polling, not listening: the
    /// decision may come from a different binary over the shared DB).
    /// Returns [`ApprovalOutcome::Denied`] — never an error — when the
    /// deadline passes: an approval timeout is a product decision (default
    /// deny), not a failure.
    pub async fn wait_for_decision(
        &self,
        tenant_id: &str,
        task_id: &str,
        timeout_ms: i64,
    ) -> Result<ApprovalOutcome, WorkflowError> {
        let deadline = now_ms() + timeout_ms;
        loop {
            let Some(task) = self.get_task(tenant_id, task_id).await? else {
                return Ok(ApprovalOutcome::Denied {
                    reason: "approval task no longer exists".into(),
                });
            };
            match task.status.as_str() {
                "approved" => return Ok(ApprovalOutcome::Approved),
                "rejected" => {
                    return Ok(ApprovalOutcome::Denied {
                        reason: task
                            .note
                            .map(|note| format!("rejected by an administrator: {note}"))
                            .unwrap_or_else(|| "rejected by an administrator".into()),
                    });
                }
                "expired" => {
                    return Ok(ApprovalOutcome::Denied {
                        reason: "approval timed out — denied by default".into(),
                    });
                }
                _ => {}
            }
            if now_ms() >= deadline {
                // Expire it ourselves so the queue stops showing a task whose
                // agent already moved on with the denial.
                sqlx::query(
                    "UPDATE one_workflow_tasks SET status = 'expired' \
                     WHERE id = ? AND status = 'pending'",
                )
                .bind(task_id)
                .execute(&self.pool)
                .await?;
                return Ok(ApprovalOutcome::Denied {
                    reason: "approval timed out — denied by default".into(),
                });
            }
            tokio::time::sleep(std::time::Duration::from_millis(APPROVAL_POLL_INTERVAL_MS)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (dream_core_db::Database, WorkflowService) {
        let db = dream_core_db::init_database_memory().await.unwrap();
        crate::migrate::run_one_workflow_migrations(db.pool()).await.unwrap();
        let service = WorkflowService::new(db.pool().clone());
        (db, service)
    }

    async fn seed_membership(pool: &sqlx::SqlitePool, user_id: &str, tenant_id: &str, role: &str) {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS one_user_org (user_id TEXT NOT NULL, tenant_id TEXT NOT NULL, \
                 role TEXT NOT NULL DEFAULT 'member', department_id TEXT, created_at INTEGER NOT NULL DEFAULT 0, \
                 updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (user_id, tenant_id));\
             CREATE TABLE IF NOT EXISTS one_active_tenant (user_id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, \
                 updated_at INTEGER NOT NULL DEFAULT 0);",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO one_user_org (user_id, tenant_id, role) VALUES (?, ?, ?)")
            .bind(user_id)
            .bind(tenant_id)
            .bind(role)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_task_validates_kind_and_title() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        assert_eq!(
            service
                .create_task("t1", "purchase_order", "u1", "t", "", &serde_json::Value::Null, None)
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service
                .create_task("t1", "prompt", "u1", "   ", "", &serde_json::Value::Null, None)
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        // All five OpenOcta-aligned kinds are accepted.
        for kind in WORKFLOW_TASK_KINDS {
            assert!(
                service
                    .create_task(
                        "t1",
                        kind,
                        "u1",
                        &format!("task {kind}"),
                        "",
                        &serde_json::Value::Null,
                        None
                    )
                    .await
                    .is_ok(),
                "kind {kind} should be accepted"
            );
        }
    }

    #[tokio::test]
    async fn pending_decided_and_mine_views_stay_disjoint() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        let a = service
            .create_task(
                "t1",
                "resource",
                "u1",
                "grant me a skill",
                "",
                &serde_json::Value::Null,
                None,
            )
            .await
            .unwrap();
        service
            .create_task(
                "t1",
                "prompt",
                "u1",
                "publish my prompt",
                "",
                &serde_json::Value::Null,
                None,
            )
            .await
            .unwrap();

        assert_eq!(service.list_tasks("t1", "pending", None, 100).await.unwrap().len(), 2);
        service
            .decide("t1", &a.id, "approved", "admin1", Some("ok"))
            .await
            .unwrap();

        let decided = service.list_tasks("t1", "decided", None, 100).await.unwrap();
        assert_eq!(decided.len(), 1);
        assert_eq!(decided[0].status, "approved");
        assert_eq!(decided[0].decided_by.as_deref(), Some("admin1"));
        let mine = service.list_tasks("t1", "mine", Some("u1"), 100).await.unwrap();
        assert_eq!(mine.len(), 2);
        // Unknown views are rejected, not silently empty.
        assert_eq!(
            service
                .list_tasks("t1", "everything", None, 100)
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
    }

    #[tokio::test]
    async fn only_a_pending_task_can_be_decided() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        let task = service
            .create_task(
                "t1",
                "creation",
                "u1",
                "ship the deck",
                "",
                &serde_json::Value::Null,
                None,
            )
            .await
            .unwrap();
        service
            .decide("t1", &task.id, "approved", "admin1", None)
            .await
            .unwrap();
        // A second decision is refused — history the hot path already saw
        // must not be rewritten.
        let err = service
            .decide("t1", &task.id, "rejected", "admin1", None)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "BAD_REQUEST");
        // Unknown decisions are refused.
        assert_eq!(
            service
                .decide("t1", "wft_other", "maybe", "admin1", None)
                .await
                .unwrap_err()
                .code(),
            "BAD_REQUEST"
        );
        assert_eq!(
            service
                .decide("t1", "wft_missing", "approved", "admin1", None)
                .await
                .unwrap_err()
                .code(),
            "NOT_FOUND"
        );
    }

    #[tokio::test]
    async fn wait_returns_the_decision_and_the_reason_travels() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        let task = service
            .create_task(
                "t1",
                "tool",
                "u1",
                "rm -rf /tmp/cache",
                "",
                &serde_json::Value::Null,
                None,
            )
            .await
            .unwrap();

        // Decision wins over the timeout even when it arrives "late" in poll
        // terms — the first read sees it.
        service
            .decide("t1", &task.id, "approved", "admin1", None)
            .await
            .unwrap();
        assert_eq!(
            service.wait_for_decision("t1", &task.id, 60_000).await.unwrap(),
            ApprovalOutcome::Approved
        );

        let rejected = service
            .create_task(
                "t1",
                "tool",
                "u1",
                "kubectl delete ns prod",
                "",
                &serde_json::Value::Null,
                None,
            )
            .await
            .unwrap();
        service
            .decide(
                "t1",
                &rejected.id,
                "rejected",
                "admin1",
                Some("use the staging cluster"),
            )
            .await
            .unwrap();
        assert_eq!(
            service.wait_for_decision("t1", &rejected.id, 60_000).await.unwrap(),
            ApprovalOutcome::Denied {
                reason: "rejected by an administrator: use the staging cluster".into()
            }
        );
    }

    #[tokio::test]
    async fn approval_timeout_denies_by_default_and_expires_the_task() {
        let (db, service) = setup().await;
        seed_membership(db.pool(), "admin1", "t1", "org_admin").await;
        let task = service
            .create_task("t1", "tool", "u1", "sudo reboot", "", &serde_json::Value::Null, None)
            .await
            .unwrap();

        assert_eq!(
            service.wait_for_decision("t1", &task.id, 1).await.unwrap(),
            ApprovalOutcome::Denied {
                reason: "approval timed out — denied by default".into()
            }
        );
        // The task is marked expired, so the queue stops showing it and the
        // 已办 view records what actually happened.
        assert_eq!(service.list_tasks("t1", "pending", None, 100).await.unwrap().len(), 0);
        let decided = service.list_tasks("t1", "decided", None, 100).await.unwrap();
        assert_eq!(decided.len(), 1);
        assert_eq!(decided[0].status, "expired");

        // A deadline-passed task is reported expired on read, never pending.
        let stale = service
            .create_task(
                "t1",
                "tool",
                "u1",
                "curl pastebin.sh",
                "",
                &serde_json::Value::Null,
                Some(1),
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        assert_eq!(
            service.get_task("t1", &stale.id).await.unwrap().unwrap().status,
            "expired"
        );
    }
}
