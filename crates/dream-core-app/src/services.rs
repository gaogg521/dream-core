//! Shared application services for dependency injection.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{AppConfig, IdentityMode, derive_encryption_key};
use dream_core_ai_agent::{
    AcpSessionSyncService, AcpSkillManager, ActiveLeaseRegistry, AgentFactoryDeps, AgentRegistry, IWorkerTaskManager,
    RuntimeTokenService, WorkerTaskManagerImpl, build_agent_factory,
};
use dream_core_auth::{
    CookieConfig, GENERATED_PASSWORD_LEN, JwtService, QrTokenStore, generate_password, hash_password,
    resolve_jwt_secret,
};
use dream_core_common::OnConversationDelete;
use dream_core_conversation::{ConversationService, runtime_state::ConversationRuntimeStateService};
use dream_core_db::{
    Database, IAcpSessionRepository, IAgentMetadataRepository, IConversationRepository, IMcpServerRepository,
    IProjectStore, ISkillRepository, IUserRepository, SqliteAcpSessionRepository, SqliteAgentMetadataRepository,
    SqliteAssistantDefinitionRepository, SqliteAssistantOverlayRepository, SqliteAssistantPreferenceRepository,
    SqliteConversationRepository, SqliteMcpServerRepository, SqliteProjectStore, SqliteProviderRepository,
    SqliteSkillRepository, SqliteUserRepository,
};
use dream_core_project::ProjectService;
use dream_core_realtime::{BroadcastEventBus, WebSocketManager};

pub struct AppServices {
    pub database: Database,
    /// P3-3: backend-agnostic handle for the enterprise `one_*` tables.
    /// `DbPool::Sqlite` in personal / enterprise-SQLite deployments; `MySql`
    /// when `DREAM_DATABASE_URL` points at a MySQL 8.0.16+ server. The main
    /// conversation schema stays on SQLite in every deployment (mixed storage
    /// by design — see the P3-3 plan §4).
    pub db: dream_core_db::DbPool,
    pub jwt_service: Arc<JwtService>,
    pub user_repo: Arc<dyn IUserRepository>,
    pub cookie_config: Arc<CookieConfig>,
    pub qr_token_store: Arc<QrTokenStore>,
    pub ws_manager: Arc<WebSocketManager>,
    pub event_bus: Arc<BroadcastEventBus>,
    pub worker_task_manager: Arc<dyn IWorkerTaskManager>,
    pub active_lease_registry: Arc<ActiveLeaseRegistry>,
    pub runtime_token_service: Arc<RuntimeTokenService>,
    pub conversation_runtime_state: Arc<ConversationRuntimeStateService>,
    pub conversation_service: ConversationService,
    /// Project-bind service (project-bind side branch). Shared by conversation
    /// and team wiring to bind/backfill project/folder rows. Cheap to clone.
    pub project_service: ProjectService,
    /// Same instance as `worker_task_manager`, exposed through the
    /// `OnConversationDelete` trait so `ConversationService::with_delete_hook`
    /// can wire it up. Optional because tests construct `AppServices` with a
    /// mock `worker_task_manager` that does not implement the trait.
    pub task_manager_delete_hook: Option<Arc<dyn OnConversationDelete>>,
    pub agent_registry: Arc<AgentRegistry>,
    pub conversation_repo: Arc<dyn IConversationRepository>,
    pub acp_session_sync: Arc<AcpSessionSyncService>,
    /// Raw JWT secret string, used for auth token signing. NOT for data
    /// encryption — auth flows rotate this to invalidate sessions.
    pub jwt_secret_raw: String,
    /// Raw data-at-rest encryption secret, used to derive the AES key for all
    /// stored API keys. Stable across auth/session-invalidation rotations.
    pub data_secret_raw: String,
    pub data_dir: PathBuf,
    pub dump_prompts: bool,
    pub work_dir: PathBuf,
    /// When `true`, skip JWT authentication and use a fixed default user.
    pub local: bool,
    pub identity_mode: IdentityMode,
    pub bootstrap_secret: Option<Arc<str>>,
    pub app_version: String,
    /// Resolved skill paths. Shared with the `ConversationService` for
    /// snapshot resolution at create time.
    pub skill_paths: Arc<dream_core_extension::SkillPaths>,
    /// User skill metadata and import history repository.
    pub skill_repo: Arc<dyn ISkillRepository>,
    /// Company content-inspection rules and the findings they produce on this
    /// machine (T4). Constructed here rather than per-router because two router
    /// states need the *same* instance: the system router loads rules into it,
    /// the conversation router enforces them.
    pub content_inspection: Arc<dream_core_system::ContentInspectionService>,
    /// Billing plane (license tier / seats / usage / model allowlist).
    ///
    /// Constructed here rather than in `routes.rs` because the agent factory —
    /// built in this function, before any router exists — needs the model
    /// allowlist to gate the `ReadImage` vision delegate. It is dependency-free
    /// (pool + manual provider), so building it early costs nothing, and the
    /// router reuses this instance instead of making a second one.
    #[cfg(feature = "enterprise")]
    pub billing: Arc<dream_domain_billing::BillingService>,
    /// Shared by every enterprise policy gate. Lives here because the agent
    /// factory takes one of those gates before any router exists, and all of
    /// them must observe the same "when did the plane last answer".
    #[cfg(feature = "enterprise")]
    pub policy_grace: Arc<crate::router::PolicyGrace>,
    backend_binary_path: Arc<PathBuf>,
    runtime_helper_bin: String,
    runtime_base_url: String,
    /// Shared with the Antigravity hook endpoint so it can authenticate callbacks.
    pub(crate) antigravity_hook_tokens: Arc<dream_core_ai_agent::antigravity_hook::HookTokenRegistry>,
}

impl AppServices {
    pub(crate) fn backend_binary_path(&self) -> Arc<PathBuf> {
        self.backend_binary_path.clone()
    }

    /// Replace the worker task manager after construction.
    ///
    /// Primarily used by tests to inject mock implementations.
    pub fn with_worker_task_manager(mut self, wtm: Arc<dyn IWorkerTaskManager>) -> Self {
        self.worker_task_manager = wtm;
        self.conversation_service = build_conversation_service(ConversationServiceDeps {
            database: &self.database,
            work_dir: self.work_dir.clone(),
            event_bus: self.event_bus.clone(),
            skill_paths: self.skill_paths.clone(),
            skill_repo: self.skill_repo.clone(),
            worker_task_manager: self.worker_task_manager.clone(),
            conversation_runtime_state: self.conversation_runtime_state.clone(),
            conversation_repo: self.conversation_repo.clone(),
            task_manager_delete_hook: self.task_manager_delete_hook.clone(),
            runtime_helper_bin: self.runtime_helper_bin.clone(),
            runtime_base_url: self.runtime_base_url.clone(),
            runtime_token_service: self.runtime_token_service.clone(),
            project_service: self.project_service.clone(),
        });
        self
    }

    pub async fn from_config(database: Database, config: &AppConfig) -> anyhow::Result<Self> {
        let backend_binary_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aioncore"));
        Self::from_config_with_backend_binary_path(database, config, backend_binary_path).await
    }

    /// Construct application services with an explicitly resolved backend binary.
    ///
    /// Runtime entry points should use [`Self::from_config`]. This variant lets
    /// integration tests run the real `dreamcore` MCP/helper subcommands instead
    /// of accidentally respawning the test harness returned by `current_exe()`.
    pub async fn from_config_with_backend_binary_path(
        database: Database,
        config: &AppConfig,
        backend_binary_path: PathBuf,
    ) -> anyhow::Result<Self> {
        // `dunce`, not `std::fs`: this path is embedded into agent-facing config
        // files (e.g. antigravity's `.agents/hooks.json` command line, executed
        // through cmd.exe on Windows), and `std::fs::canonicalize` on Windows
        // returns a `\\?\`-prefixed verbatim path that cmd.exe cannot launch.
        // `dunce::canonicalize` resolves symlinks the same way but keeps the
        // plain drive-letter form whenever the path is representable without
        // the prefix.
        let backend_binary_path = dunce::canonicalize(&backend_binary_path).unwrap_or(backend_binary_path);
        let data_dir = config.data_dir.clone();
        let work_dir = config.work_dir.clone();
        let identity_mode = config.effective_identity_mode();
        let local = identity_mode.is_local();
        let dump_prompts = config.dump_prompts;
        let app_version = config.app_version.clone();
        let user_repo: Arc<dyn IUserRepository> = Arc::new(SqliteUserRepository::new(database.pool().clone()));

        // Resolve JWT secret: env var → system user db field → random generation
        let env_secret = std::env::var("JWT_SECRET").ok();
        let system_user = user_repo
            .get_system_user()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get system user: {e}"))?;

        let db_secret = system_user
            .as_ref()
            .and_then(|u| u.jwt_secret.as_deref())
            .filter(|s| !s.is_empty());

        let (secret, is_new) = resolve_jwt_secret(env_secret.as_deref(), db_secret);

        // Defense-in-depth for the encryption key: generating a NEW secret is
        // only legitimate on a genuinely fresh install. If the read path
        // claimed "no system user" while the row actually exists (as happened
        // when a stale post-migration connection mis-decoded the users table,
        // ELECTRON-3T0), deriving a fresh key would silently break decryption
        // of every stored credential. Verify absence with an independent
        // query and fail startup instead of corrupting.
        if is_new
            && system_user.is_none()
            && user_repo
                .find_by_id("system_default_user")
                .await
                .map_err(|e| anyhow::anyhow!("Failed to verify system user absence: {e}"))?
                .is_some()
        {
            anyhow::bail!(
                "system user row exists but could not be read; refusing to generate a new                  JWT secret (would break decryption of stored credentials)"
            );
        }

        // Persist newly generated secret to database
        if is_new && let Some(user) = &system_user {
            user_repo
                .update_jwt_secret(&user.id, &secret)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to persist JWT secret: {e}"))?;
            tracing::info!("Generated and persisted new JWT secret");
        }

        // Resolve the data-at-rest encryption secret. This is deliberately
        // SEPARATE from `jwt_secret`: auth flows (org join/leave/create/reset,
        // password change) rotate `jwt_secret` to invalidate sessions, and if
        // the encryption key were derived from it, every such rotation would
        // silently orphan all stored provider/team/mcp/channel API keys.
        //
        // Back-compat: existing installs have no `data_secret` yet. Seed it
        // from the CURRENT `jwt_secret` so any key still decryptable with the
        // current jwt-derived key stays readable after the upgrade. From then
        // on `data_secret` is stable and untouched by auth rotations. Keys
        // that were already orphaned by a past rotation cannot be recovered.
        let db_data_secret = system_user
            .as_ref()
            .and_then(|u| u.data_secret.as_deref())
            .filter(|s| !s.is_empty());
        let data_secret = match db_data_secret {
            Some(existing) => existing.to_owned(),
            None => {
                if let Some(user) = &system_user {
                    user_repo
                        .update_data_secret(&user.id, &secret)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to persist data secret: {e}"))?;
                    tracing::info!("Seeded data encryption secret from current JWT secret (one-time backfill)");
                }
                secret.clone()
            }
        };

        // Enterprise first-boot bootstrap: a fresh non-local (Webui/DreamPro)
        // deployment's seed admin account (`system_default_user`) has an empty
        // `password_hash` — `login_handler` treats that as "invalid
        // credentials", so with nothing else to set a password there is no
        // way to ever log in. `--local` desktop mode is unaffected (it never
        // needs a password: `auth_middleware` resolves the operator without
        // one) and already has its own in-app password reset
        // (`POST /api/webui/reset-password`).
        //
        // Runs at most once per install: after this, `password_hash` is no
        // longer empty, so a restart takes the `db_secret`-populated path
        // above and skips straight past this block without regenerating or
        // overwriting whatever password is there (including one the operator
        // has since chosen via change-password).
        if !local
            && let Some(user) = &system_user
            && user.password_hash.as_deref().unwrap_or("").is_empty()
        {
            let plaintext = generate_password(GENERATED_PASSWORD_LEN);
            let new_hash = {
                let p = plaintext.clone();
                tokio::task::spawn_blocking(move || hash_password(&p))
                    .await
                    .map_err(|e| anyhow::anyhow!("Task join error: {e}"))?
                    .map_err(|e| anyhow::anyhow!("Failed to hash initial admin password: {e}"))?
            };
            user_repo
                .update_password(&user.id, &new_hash)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to persist initial admin password: {e}"))?;
            user_repo
                .set_must_change_password(&user.id, true)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to flag initial admin password for change: {e}"))?;

            let username = user.username.clone().unwrap_or_default();

            // The structured `tracing::warn!` below is one line among the
            // dozen other startup log lines this function already emits —
            // fine for an operator who knows to `grep` their logs, a real
            // barrier for one who does not. Two more channels, aimed at that
            // person specifically:
            //
            // 1. A plain-text file in the data directory. Whoever configured
            //    that volume mount can browse to it with an ordinary file
            //    manager — no log-reading or Docker CLI knowledge required.
            //    Best-effort: a write failure here must not block startup,
            //    the tracing line below is still emitted regardless.
            let password_file = data_dir.join("INITIAL_ADMIN_PASSWORD.txt");
            let file_contents = format!(
                "This file was generated once, the first time this server started with no \
                 admin password set.\n\n\
                 Username: {username}\n\
                 Password: {plaintext}\n\n\
                 Log in with this password, then change it immediately — you will be required \
                 to before you can do anything else. This file is not updated again after that; \
                 it is safe to delete once you have changed your password.\n"
            );
            if let Err(e) = std::fs::write(&password_file, file_contents) {
                tracing::warn!(
                    path = %password_file.display(),
                    error = %e,
                    "could not write the initial admin password file — it was still logged below"
                );
            }

            // 2. A banner on stdout, deliberately NOT going through the
            // structured `key=value` tracing format so it reads as plain
            // text at a glance instead of one more line to parse.
            println!(
                "\n\
                 ================================================================\n\
                   Generated an initial admin password for this deployment.\n\
                 \n\
                   Username: {username}\n\
                   Password: {plaintext}\n\
                 \n\
                   Log in with this password, then change it immediately — you\n\
                   will be required to before doing anything else. It will not\n\
                   be shown again (also saved to {path}).\n\
                 ================================================================\n",
                path = password_file.display()
            );

            tracing::warn!(
                username = %username,
                password = %plaintext,
                "generated an initial admin password for this deployment — log in and change it \
                 immediately; it will not be shown again"
            );
        }

        let encryption_key = derive_encryption_key(&data_secret);

        let provider_repo = Arc::new(SqliteProviderRepository::new(database.pool().clone()));
        let event_bus = Arc::new(BroadcastEventBus::new(256));
        // User-configured MCP servers — injected into ACP `session/new`
        // so the agent gets the operator's tools (ELECTRON-1JG fix).
        let mcp_server_repo: Arc<dyn IMcpServerRepository> =
            Arc::new(SqliteMcpServerRepository::new(database.pool().clone()));
        let codex_bridge_config_repo: Arc<dyn dream_core_db::ICodexBridgeConfigRepository> = Arc::new(
            dream_core_db::SqliteCodexBridgeConfigRepository::new(database.pool().clone()),
        );
        let claude_bridge_config_repo: Arc<dyn dream_core_db::IClaudeBridgeConfigRepository> = Arc::new(
            dream_core_db::SqliteClaudeBridgeConfigRepository::new(database.pool().clone()),
        );

        let agent_metadata_repo: Arc<dyn IAgentMetadataRepository> =
            Arc::new(SqliteAgentMetadataRepository::new(database.pool().clone()));
        let agent_registry = AgentRegistry::new(agent_metadata_repo);
        agent_registry
            .hydrate()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to hydrate agent registry: {e}"))?;
        // Settle any slow version probes off the readiness path (#675):
        // hydrate never waits beyond the inline budget per agent.
        agent_registry.spawn_slow_probe_recheck();

        let acp_session_repo: Arc<dyn IAcpSessionRepository> =
            Arc::new(SqliteAcpSessionRepository::new(database.pool().clone()));
        let acp_agent_service = AcpSessionSyncService::new(acp_session_repo.clone());

        let conversation_repo: Arc<dyn IConversationRepository> =
            Arc::new(SqliteConversationRepository::new(database.pool().clone()));
        let skill_repo: Arc<dyn ISkillRepository> = Arc::new(SqliteSkillRepository::new(database.pool().clone()));

        // Project-bind service (side branch). temp_root mirrors the existing
        // conversation temp-workspace root (`work_dir/conversations`) so
        // `resolve_existing` classifies auto workspaces as temp and
        // user-picked directories as standard.
        let project_store: Arc<dyn IProjectStore> = Arc::new(SqliteProjectStore::new(database.pool().clone()));
        let project_service = ProjectService::new(project_store, work_dir.join("conversations"));

        // Skill paths need app resource dir (for builtin rules) + data dir
        // (for user skills + materialized views). AcpSkillManager uses these
        // for first-message skill index/body loading.
        let app_resource_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.canonicalize().ok())
            .and_then(|p| p.parent().map(|pp| pp.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let skill_paths = Arc::new(dream_core_extension::resolve_skill_paths(&app_resource_dir, &data_dir));
        if identity_mode.is_local() {
            dream_core_extension::sync_skill_catalog_into_repo(skill_paths.as_ref(), skill_repo.as_ref())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to synchronize skill catalog: {e}"))?;
        } else {
            // DreamPro: never ingest the legacy shared skill directory — its
            // files carry no account attribution and would only create rows
            // for the never-logged-in local default user.
            dream_core_extension::sync_builtin_skill_catalog_into_repo(skill_paths.as_ref(), skill_repo.as_ref())
                .await
                .map_err(|e| anyhow::anyhow!("Failed to synchronize skill catalog: {e}"))?;
        }

        // Absolute path to this process's binary. Reused as the `command` for
        // the stdio MCP bridge spawned by ACP CLIs when a team session is
        // attached to a conversation (phase1 mcp.md §4.6 single-binary model).
        let backend_binary_path = Arc::new(backend_binary_path);
        let runtime_helper_bin = backend_binary_path.to_string_lossy().into_owned();
        let runtime_base_url = config.local_base_url();
        let antigravity_hook_tokens = Arc::new(dream_core_ai_agent::antigravity_hook::HookTokenRegistry::new());

        // Subprocess spawner for the direct-CLI `SessionAgentTask`. Registry-backed
        // (feature 001) so spawned processes are reap-gateable; a fresh per-run epoch,
        // since no cross-run reap authority is required for this spawn path.
        //
        // ⚠️ Fork divergence: upstream wires this because claude/codex always take the
        // direct-CLI path. Here it serves **Antigravity only** — `agy` has no ACP
        // surface, so it has nowhere else to run. claude/codex stay on the ACP manager
        // so they keep the first-party Codex/Claude bridge; see
        // `dream_core_ai_agent::factory::acp::route_for_backend`.
        let process_registry = Arc::new(dream_core_process::FileRegistryStore::new(&data_dir));
        let machine_id = dream_core_process::local_machine_id(&data_dir);
        let session_spawner: Arc<dyn dream_core_process::Spawner> = Arc::new(dream_core_process::RealSpawner::new(
            process_registry,
            uuid::Uuid::now_v7(),
            machine_id,
        ));

        // Billing plane. Built before the agent factory so the factory can take
        // the model allowlist: picking the `ReadImage` vision delegate is a
        // model choice the send-path gates never see (they only ever look at
        // the *session* model), so without this an admin could remove a model
        // from the allowlist and still have it invoked as a delegate.
        // P3-3: enterprise main storage backend. `DREAM_DATABASE_URL` starting
        // with `mysql://` connects the enterprise `one_*` tables to MySQL;
        // anything else (unset included) keeps them in the SQLite database.
        // Inert on personal builds.
        let db = match std::env::var("DREAM_DATABASE_URL")
            .ok()
            .filter(|u| u.starts_with("mysql://"))
        {
            #[cfg(feature = "enterprise")]
            Some(url) => {
                let mysql_db = dream_core_db::init_database_mysql(&url).await?;
                dream_core_db::DbPool::MySql(mysql_db.pool().clone())
            }
            #[cfg(not(feature = "enterprise"))]
            Some(_) => {
                anyhow::bail!("DREAM_DATABASE_URL MySQL storage requires the enterprise build");
            }
            None => dream_core_db::DbPool::Sqlite(database.pool().clone()),
        };

        #[cfg(feature = "enterprise")]
        let policy_grace = Arc::new(crate::router::PolicyGrace::new());
        #[cfg(feature = "enterprise")]
        let billing = Arc::new(dream_domain_billing::BillingService::new(
            db.clone(),
            Arc::new(dream_domain_billing::ManualBillingProvider),
        ));
        // Own instance, same posture as `billing` above: cheap to construct
        // (pool clone + key), needed here so the agent factory can take the
        // destructive-command gate for the ACP permission router.
        #[cfg(feature = "enterprise")]
        let platform_for_agent_factory = Arc::new(dream_domain_platform::PlatformService::new(
            db.clone(),
            encryption_key,
        ));
        // Same posture as `platform_for_agent_factory` above: a second
        // WorkflowService over the same pool is harmless (pool clone + no
        // in-memory state), and the terminal-tool approval gate needs one
        // here because the agent factory is built before the router — and
        // before the governance plane that also holds a handle.
        #[cfg(feature = "enterprise")]
        let workflow_for_agent_factory = Arc::new(dream_domain_workflow::WorkflowService::new(db.clone()));

        // NOT adopted this sync (2026-07-29): upstream wires a `session_spawner`
        // here for the direct-CLI SessionAgentTask path — see the matching notes
        // in factory/mod.rs and factory/acp.rs for why (it would bypass our
        // first-party Codex/Claude bridge). Nothing else in this file needs the
        // registry/machine-id/spawner trio upstream added, so they're dropped
        // rather than kept unused.
        let factory = build_agent_factory(AgentFactoryDeps {
            skill_manager: AcpSkillManager::new_with_repo(skill_paths.clone(), skill_repo.clone()),
            provider_repo,
            encryption_key,
            agent_registry: agent_registry.clone(),
            acp_agent_service: acp_agent_service.clone(),
            data_dir: data_dir.clone(),
            dump_prompts,
            broadcaster: event_bus.clone(),
            backend_binary_path: backend_binary_path.clone(),
            mcp_server_repo: Some(mcp_server_repo),
            codex_bridge_config_repo: Some(codex_bridge_config_repo),
            local_base_url: runtime_base_url.clone(),
            claude_bridge_config_repo: Some(claude_bridge_config_repo),
            session_spawner,
            // agy cannot prompt for tool permission in headless mode, so Dream UI
            // registers itself as its PreToolUse hook; the hook process calls
            // back here to raise the user's permission card.
            antigravity_hook_base_url: Some(runtime_base_url.clone()),
            antigravity_hook_tokens: antigravity_hook_tokens.clone(),
            // Personal edition has no allowlist to enforce: `None` means the
            // vision delegate picks whatever model the session resolved to,
            // which is the pre-billing behaviour.
            #[cfg(feature = "enterprise")]
            model_allowlist: Some(Arc::new(crate::router::BillingModelAllowlistGate {
                billing: billing.clone(),
                grace: policy_grace.clone(),
            })),
            #[cfg(not(feature = "enterprise"))]
            model_allowlist: None,
            // Personal edition has no security policy to enforce: `None`
            // means every ACP permission request flows through unmodified,
            // exactly as before this existed.
            #[cfg(feature = "enterprise")]
            tool_call_security_gate: Some(Arc::new(crate::router::PlatformToolCallSecurityGate {
                platform: platform_for_agent_factory,
                workflow: Some(workflow_for_agent_factory),
            })),
            #[cfg(not(feature = "enterprise"))]
            tool_call_security_gate: None,
            // Enterprise memory recall (P2-2 §B.4 完整版): a per-turn ACP
            // prompt hook injects the caller's readable memory into every
            // prompt. `None` in personal builds — the hook is not registered.
            #[cfg(feature = "enterprise")]
            memory_recall: Some(Arc::new(crate::router::OneMemoryContextProvider {
                memory: Arc::new(dream_domain_memory::MemoryService::new(db.clone())),
            }) as Arc<dyn dream_core_ai_agent::TurnMemoryRecall>),
            #[cfg(not(feature = "enterprise"))]
            memory_recall: None,
        });

        // Agent factory is now wired. Future extension/custom agents
        // that get written to `agent_metadata` will show up after the
        // relevant service calls `AgentRegistry::hydrate`.
        let active_lease_registry = Arc::new(ActiveLeaseRegistry::new());
        let runtime_token_service = Arc::new(RuntimeTokenService::new());
        let task_manager_concrete = Arc::new(
            WorkerTaskManagerImpl::new_with_active_leases(factory, active_lease_registry.clone())
                .with_runtime_token_service(runtime_token_service.clone()),
        );
        let worker_task_manager: Arc<dyn IWorkerTaskManager> = task_manager_concrete.clone();
        let task_manager_delete_hook: Arc<dyn OnConversationDelete> = task_manager_concrete;
        let conversation_runtime_state = Arc::new(ConversationRuntimeStateService::default());
        let conversation_service = build_conversation_service(ConversationServiceDeps {
            database: &database,
            work_dir: work_dir.clone(),
            event_bus: event_bus.clone(),
            skill_paths: skill_paths.clone(),
            skill_repo: skill_repo.clone(),
            worker_task_manager: worker_task_manager.clone(),
            conversation_runtime_state: conversation_runtime_state.clone(),
            conversation_repo: conversation_repo.clone(),
            task_manager_delete_hook: Some(task_manager_delete_hook.clone()),
            runtime_helper_bin: runtime_helper_bin.clone(),
            runtime_base_url: runtime_base_url.clone(),
            runtime_token_service: runtime_token_service.clone(),
            project_service: project_service.clone(),
        });

        

        Ok(Self {
            database,
            db,
            #[cfg(feature = "enterprise")]
            billing,
            #[cfg(feature = "enterprise")]
            policy_grace,
            jwt_service: Arc::new(JwtService::new(secret.clone())),
            antigravity_hook_tokens,
            user_repo,
            cookie_config: Arc::new(CookieConfig::from_env()),
            qr_token_store: Arc::new(QrTokenStore::new()),
            ws_manager: Arc::new(WebSocketManager::new()),
            event_bus,
            worker_task_manager,
            active_lease_registry,
            runtime_token_service,
            conversation_runtime_state,
            conversation_service,
            project_service,
            task_manager_delete_hook: Some(task_manager_delete_hook),
            agent_registry,
            conversation_repo,
            acp_session_sync: acp_agent_service,
            jwt_secret_raw: secret,
            data_secret_raw: data_secret,
            data_dir,
            dump_prompts,
            work_dir,
            local,
            identity_mode,
            bootstrap_secret: config.bootstrap_secret.clone().map(Arc::<str>::from),
            app_version,
            skill_paths,
            skill_repo,
            content_inspection: Arc::new(dream_core_system::ContentInspectionService::new()),
            backend_binary_path,
            runtime_helper_bin,
            runtime_base_url,
        })
    }
}

struct ConversationServiceDeps<'a> {
    database: &'a Database,
    work_dir: PathBuf,
    event_bus: Arc<BroadcastEventBus>,
    skill_paths: Arc<dream_core_extension::SkillPaths>,
    skill_repo: Arc<dyn ISkillRepository>,
    worker_task_manager: Arc<dyn IWorkerTaskManager>,
    conversation_runtime_state: Arc<ConversationRuntimeStateService>,
    conversation_repo: Arc<dyn IConversationRepository>,
    task_manager_delete_hook: Option<Arc<dyn OnConversationDelete>>,
    runtime_helper_bin: String,
    runtime_base_url: String,
    runtime_token_service: Arc<RuntimeTokenService>,
    project_service: ProjectService,
}

fn build_conversation_service(deps: ConversationServiceDeps<'_>) -> ConversationService {
    let skill_resolver = Arc::new(dream_core_conversation::skill_resolver::ExtensionSkillResolver::new(
        deps.skill_paths,
        deps.skill_repo,
    ));
    let service = ConversationService::new(
        deps.work_dir,
        deps.event_bus,
        skill_resolver,
        deps.worker_task_manager,
        deps.conversation_repo,
        Arc::new(SqliteAgentMetadataRepository::new(deps.database.pool().clone())),
        Arc::new(SqliteAcpSessionRepository::new(deps.database.pool().clone())),
    )
    .with_runtime_state(deps.conversation_runtime_state)
    .with_runtime_helper_context(deps.runtime_helper_bin, deps.runtime_base_url)
    .with_runtime_token_service(deps.runtime_token_service);
    service.with_mcp_server_repo(Arc::new(SqliteMcpServerRepository::new(deps.database.pool().clone())));
    service.with_assistant_definition_repo(Arc::new(SqliteAssistantDefinitionRepository::new(
        deps.database.pool().clone(),
    )));
    service.with_assistant_state_repo(Arc::new(SqliteAssistantOverlayRepository::new(
        deps.database.pool().clone(),
    )));
    service.with_assistant_preference_repo(Arc::new(SqliteAssistantPreferenceRepository::new(
        deps.database.pool().clone(),
    )));
    if let Some(hook) = deps.task_manager_delete_hook {
        service.with_delete_hook(hook);
    }
    service.with_project_service(Arc::new(deps.project_service));
    service
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_app_services_from_memory_db() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let services = AppServices::from_config(db, &AppConfig::default()).await.unwrap();

        // JWT service should be functional
        let token = services.jwt_service.sign("test_user", "testuser").unwrap();
        let payload = services.jwt_service.verify(&token).unwrap();
        assert_eq!(payload.user_id, "test_user");

        // User repo should have system user. `AppConfig::default()` is
        // non-local, so the first-boot bootstrap above already gave it a real
        // (system-generated) password — `has_users()` no longer excludes it
        // the way it would for a genuinely empty-password seed account.
        let has_users = services.user_repo.has_users().await.unwrap();
        assert!(has_users);

        services.database.close().await;
    }

    #[tokio::test]
    async fn test_jwt_secret_persisted_to_db() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let services = AppServices::from_config(db, &AppConfig::default()).await.unwrap();

        // System user should now have a jwt_secret persisted
        let system_user = services.user_repo.get_system_user().await.unwrap();
        let jwt_secret = system_user.unwrap().jwt_secret;
        assert!(jwt_secret.is_some());
        assert!(!jwt_secret.unwrap().is_empty());

        services.database.close().await;
    }

    #[tokio::test]
    async fn data_secret_is_seeded_from_jwt_on_first_init() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let services = AppServices::from_config(db, &AppConfig::default()).await.unwrap();

        // On a fresh install the data secret is backfilled from the current
        // jwt secret so pre-existing (jwt-encrypted) data stays decryptable.
        assert!(!services.data_secret_raw.is_empty());
        assert_eq!(services.data_secret_raw, services.jwt_secret_raw);

        // And it is persisted to the users row.
        let system_user = services.user_repo.get_system_user().await.unwrap().unwrap();
        assert_eq!(
            system_user.data_secret.as_deref(),
            Some(services.data_secret_raw.as_str())
        );

        services.database.close().await;
    }

    #[tokio::test]
    async fn data_secret_survives_jwt_secret_rotation() {
        // Regression: org join/leave/create/reset and password change rotate
        // the jwt secret to invalidate sessions. The data-encryption key must
        // NOT follow it, otherwise every such rotation orphans stored API keys.
        let db = dream_core_db::init_database_memory().await.unwrap();
        let services = AppServices::from_config(db, &AppConfig::default()).await.unwrap();
        let original_data_secret = services.data_secret_raw.clone();

        // Simulate an auth/session-invalidation rotation (as one-org does).
        services
            .user_repo
            .update_jwt_secret("system_default_user", "a-freshly-rotated-jwt-secret")
            .await
            .unwrap();

        let system_user = services.user_repo.get_system_user().await.unwrap().unwrap();
        assert_eq!(
            system_user.jwt_secret.as_deref(),
            Some("a-freshly-rotated-jwt-secret"),
            "jwt secret should have rotated"
        );
        assert_eq!(
            system_user.data_secret.as_deref(),
            Some(original_data_secret.as_str()),
            "data secret must be untouched by jwt rotation"
        );

        services.database.close().await;
    }

    #[tokio::test]
    async fn test_app_services_uses_supplied_app_version() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        let config = AppConfig {
            app_version: "9.9.9".to_string(),
            ..Default::default()
        };
        let services = AppServices::from_config(db, &config).await.unwrap();

        assert_eq!(services.app_version, "9.9.9");

        services.database.close().await;
    }

    #[tokio::test]
    async fn backend_binary_path_never_carries_a_windows_verbatim_prefix() {
        // The resolved path is embedded into agent-facing config files —
        // antigravity's `.agents/hooks.json` command line is executed through
        // cmd.exe on Windows, which cannot launch `\\?\`-prefixed programs
        // (iOfficeAI/Dream UI#4095). `std::fs::canonicalize` returns exactly
        // that form on Windows, so the constructor must keep the plain
        // drive-letter form while still resolving symlinks.
        let db = dream_core_db::init_database_memory().await.unwrap();
        let exe = std::env::current_exe().unwrap();
        let services = AppServices::from_config_with_backend_binary_path(db, &AppConfig::default(), exe)
            .await
            .unwrap();

        let resolved = services.backend_binary_path();
        assert!(
            resolved.is_absolute(),
            "canonicalization must still yield an absolute path"
        );
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "backend binary path must stay cmd.exe-launchable, got {}",
            resolved.display()
        );

        services.database.close().await;
    }
}
