//! Retire an ACP process when one of its injected company MCP grants is revoked.
//! This enforces our process boundary; it does not assume a CLI supports hot
//! replacement of session MCP lists. Verified lifecycle: capability/cli_process/mod.rs.
use super::AcpAgentManager;
use crate::{error::AgentError, factory::session_mcp::is_team_mcp_row};
use dream_core_db::IMcpServerRepository;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex as StdMutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const CHECK_INTERVAL: Duration = Duration::from_secs(1);
const REFUSAL: &str =
    "Company MCP access changed or could not be verified. Restart the conversation runtime to refresh its tools.";

pub(super) struct TeamMcpGuard {
    access: Arc<GrantCache>,
    ids: HashSet<String>,
    retired: AtomicBool,
}

struct GrantCache {
    repo: Arc<dyn IMcpServerRepository>,
    user: String,
    state: tokio::sync::Mutex<Option<(Instant, Arc<HashSet<String>>)>>,
}

type CacheKey = (usize, String);
type CacheRegistry = StdMutex<HashMap<CacheKey, Weak<GrantCache>>>;
static GRANT_CACHES: OnceLock<CacheRegistry> = OnceLock::new();

async fn watch_revocation<F, Fut>(
    guard: Arc<TeamMcpGuard>,
    process: Weak<crate::capability::cli_process::CliAgentProcess>,
    retire: F,
) where
    F: FnOnce(AgentError) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    loop {
        tokio::time::sleep(CHECK_INTERVAL).await;
        if !process.upgrade().is_some_and(|p| p.is_running()) {
            return;
        }
        if let Err(error) = guard.check(false).await {
            retire(error).await;
            return;
        }
    }
}

impl GrantCache {
    async fn active(&self, fresh: bool) -> Arc<HashSet<String>> {
        let mut state = self.state.lock().await;
        if !fresh
            && let Some((at, rows)) = state.as_ref()
            && at.elapsed() < CHECK_INTERVAL
        {
            return rows.clone();
        }
        let rows = tokio::time::timeout(Duration::from_secs(2), self.repo.list(&self.user)).await;
        // Fail closed on lookup errors, and share that result with all watchers.
        let active = match rows {
            Ok(Ok(rows)) => rows
                .into_iter()
                .filter(|r| r.enabled && r.deleted_at.is_none() && is_team_mcp_row(r))
                .map(|r| r.id)
                .collect(),
            _ => HashSet::new(),
        };
        let active = Arc::new(active);
        *state = Some((Instant::now(), active.clone()));
        active
    }
}

impl TeamMcpGuard {
    fn new(repo: Arc<dyn IMcpServerRepository>, user: String, ids: HashSet<String>) -> Self {
        let key = (Arc::as_ptr(&repo) as *const () as usize, user.clone());
        let mut registry = GRANT_CACHES
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry.retain(|_, value| value.strong_count() > 0);
        let access = registry.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
            let access = Arc::new(GrantCache {
                repo,
                user,
                state: tokio::sync::Mutex::new(None),
            });
            registry.insert(key, Arc::downgrade(&access));
            access
        });
        Self {
            access,
            ids,
            retired: AtomicBool::new(false),
        }
    }

    async fn check(&self, fresh: bool) -> Result<(), AgentError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(AgentError::forbidden(REFUSAL));
        }
        // Background watchers share one query per repository/user/interval.
        // A new outgoing prompt always forces a live read.
        // Bound queueing behind other sessions as well as the database request.
        let allowed = tokio::time::timeout(Duration::from_secs(3), self.access.active(fresh))
            .await
            .is_ok_and(|active| self.ids.is_subset(active.as_ref()));
        if !allowed {
            self.retired.store(true, Ordering::Release);
            return Err(AgentError::forbidden(REFUSAL));
        }
        Ok(())
    }
}

impl AcpAgentManager {
    pub(crate) async fn install_team_mcp_guard(
        self: &Arc<Self>,
        repo: Arc<dyn IMcpServerRepository>,
        ids: HashSet<String>,
    ) -> Result<(), AgentError> {
        if ids.is_empty() {
            return Ok(());
        }
        let guard = Arc::new(TeamMcpGuard::new(repo, self.params.user_id.clone(), ids));
        // Cover a revocation between factory snapshot construction and warmup.
        guard.check(true).await?;
        *self.team_mcp_guard.write().await = Some(guard.clone());
        let weak = Arc::downgrade(self);
        let process = Arc::downgrade(&self.process);
        tokio::spawn(watch_revocation(guard, process, move |error| async move {
            if let Some(agent) = weak.upgrade() {
                tracing::warn!(conversation_id = %agent.params.conversation_id, "Retiring ACP runtime after company MCP revocation");
                agent.runtime.emit_error(error.to_string());
                agent.kill_and_wait(None).await;
            }
        }));
        Ok(())
    }

    pub(super) async fn check_team_mcp_access(&self) -> Result<(), AgentError> {
        let guard = self.team_mcp_guard.read().await.clone();
        if let Some(guard) = guard
            && let Err(error) = guard.check(true).await
        {
            self.runtime.emit_error(error.to_string());
            self.kill_and_wait(None).await;
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_db::{CreateMcpServerParams, UpdateMcpServerParams};

    #[tokio::test]
    async fn held_grants_follow_live_db_and_retirement_cannot_be_undone_by_restore() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let repo = Arc::new(dream_core_db::SqliteMcpServerRepository::new(
            db.pool().clone(),
            [3; 32],
        ));
        let user = "system_default_user";
        let row = repo
            .create(CreateMcpServerParams {
                user_id: user,
                name: "company-held",
                description: None,
                enabled: true,
                transport_type: "stdio",
                transport_config: r#"{"command":"node"}"#,
                tools: None,
                original_json: Some(r#"{"teamRegistryId":"qa-grant"}"#),
                builtin: false,
            })
            .await
            .unwrap();
        let guard = TeamMcpGuard::new(repo.clone(), user.into(), HashSet::from([row.id.clone()]));
        let sibling = TeamMcpGuard::new(repo.clone(), user.into(), HashSet::from([row.id.clone()]));
        assert!(
            Arc::ptr_eq(&guard.access, &sibling.access),
            "sessions must share the background query cache"
        );
        assert!(guard.check(true).await.is_ok());
        repo.update(
            user,
            &row.id,
            UpdateMcpServerParams {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(matches!(guard.check(true).await, Err(AgentError::Forbidden(_))));
        repo.update(
            user,
            &row.id,
            UpdateMcpServerParams {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            guard.check(true).await.is_err(),
            "a retired process must not reuse cached credentials after restore"
        );
        let fresh = TeamMcpGuard::new(repo.clone(), user.into(), HashSet::from([row.id.clone()]));
        assert!(fresh.check(true).await.is_ok());
        repo.delete(user, &row.id).await.unwrap();
        assert!(fresh.check(true).await.is_err());
    }

    #[tokio::test]
    async fn other_users_missing_grants_and_database_errors_fail_closed() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let repo = Arc::new(dream_core_db::SqliteMcpServerRepository::new(
            db.pool().clone(),
            [4; 32],
        ));
        let guard = TeamMcpGuard::new(repo.clone(), "someone-else".into(), HashSet::from(["absent".into()]));
        assert!(guard.check(true).await.is_err());
        let guard = TeamMcpGuard::new(
            repo.clone(),
            "system_default_user".into(),
            HashSet::from(["absent".into()]),
        );
        db.pool().close().await;
        assert!(guard.check(true).await.is_err());
    }

    #[tokio::test]
    async fn idle_process_is_actually_reaped_after_revocation_without_another_prompt() {
        use crate::capability::cli_process::CliAgentProcess;
        let db = dream_core_db::init_database_memory().await.unwrap();
        let repo = Arc::new(dream_core_db::SqliteMcpServerRepository::new(
            db.pool().clone(),
            [5; 32],
        ));
        let row = repo
            .create(CreateMcpServerParams {
                user_id: "system_default_user",
                name: "held-runtime",
                description: None,
                enabled: true,
                transport_type: "stdio",
                transport_config: r#"{"command":"unused"}"#,
                tools: None,
                original_json: Some(r#"{"teamRegistryId":"runtime-grant"}"#),
                builtin: false,
            })
            .await
            .unwrap();
        let guard = Arc::new(TeamMcpGuard::new(
            repo.clone(),
            "system_default_user".into(),
            HashSet::from([row.id.clone()]),
        ));
        guard.check(true).await.unwrap();
        // A real inert OS process, not an emulated ACP provider or a model call.
        #[cfg(windows)]
        let (command, args) = (
            "powershell.exe",
            vec!["-NoLogo", "-NoProfile", "-Command", "Start-Sleep -Seconds 30"],
        );
        #[cfg(not(windows))]
        let (command, args) = ("sh", vec!["-c", "sleep 30"]);
        let process = Arc::new(
            CliAgentProcess::spawn_for_sdk(dream_core_common::CommandSpec {
                command: command.into(),
                args: args.into_iter().map(String::from).collect(),
                env: vec![],
                cwd: None,
            })
            .await
            .unwrap(),
        );
        assert!(process.is_running());
        let weak = Arc::downgrade(&process);
        let monitor = tokio::spawn(watch_revocation(guard, weak.clone(), move |_| async move {
            if let Some(process) = weak.upgrade() {
                process.kill(Duration::from_millis(10)).await.unwrap();
            }
        }));
        repo.update(
            "system_default_user",
            &row.id,
            UpdateMcpServerParams {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(10), monitor).await;
        // Always reap the test-owned process, including on an assertion failure.
        process.kill(Duration::from_millis(10)).await.unwrap();
        completed.expect("revocation watcher must finish").unwrap();
        assert!(!process.is_running());
    }
}
