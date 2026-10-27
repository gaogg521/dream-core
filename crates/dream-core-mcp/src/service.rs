use std::sync::Arc;

use dream_core_api_types::{
    BatchImportMcpServersRequest, CreateMcpServerRequest, McpConnectionTestResult, McpServerResponse,
    UpdateMcpServerRequest,
};
use dream_core_common::now_ms;
use dream_core_db::{CreateMcpServerParams, IMcpServerRepository, UpdateMcpServerParams};
use tracing::{info, warn};

use crate::error::McpError;
use crate::types::{McpServer, McpServerTransport};

const SPLITTABLE_STDIO_LAUNCHERS: &[&str] = &["npx", "pnpx", "bunx", "uvx", "uv", "node", "python", "python3", "deno"];

// ---------------------------------------------------------------------------
// McpConfigService
// ---------------------------------------------------------------------------

/// MCP server configuration CRUD service.
///
/// Handles create/read/update/delete operations on MCP server configs,
/// delegating persistence to `IMcpServerRepository`. Business rules:
///
/// - **add**: upsert by name (existing → update, new → create)
/// - **delete**: removes the stored MCP definition
/// - **toggle**: flips enabled state
/// - **batch_import**: sequential upsert by name
#[derive(Clone)]
pub struct McpConfigService {
    repo: Arc<dyn IMcpServerRepository>,
    /// Last team sync answered per user, so an identical request that arrives
    /// again within [`TEAM_SYNC_MEMO_TTL`] costs no database round trips.
    recent_team_sync: Arc<std::sync::Mutex<std::collections::HashMap<String, RecentTeamSync>>>,
    selection_mutation: Arc<tokio::sync::Mutex<()>>,
}

/// How long an identical team sync request is answered from memory. The
/// client re-posts the same registry on every team switch and reload (71 times
/// in one day on the dev machine), each costing a lookup per connector.
const TEAM_SYNC_MEMO_TTL: std::time::Duration = std::time::Duration::from_secs(60);

struct RecentTeamSync {
    digest: u64,
    at: std::time::Instant,
    report: TeamMcpSyncReport,
}

fn team_sync_digest(payloads: &[TeamMcpPayload], authoritative: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    authoritative.hash(&mut hasher);
    for p in payloads {
        p.registry_id.hash(&mut hasher);
        p.name.hash(&mut hasher);
        p.server_type.hash(&mut hasher);
        p.endpoint.hash(&mut hasher);
        p.enabled.hash(&mut hasher);
        p.employee_bound.hash(&mut hasher);
        p.secrets_json.hash(&mut hasher);
    }
    hasher.finish()
}

struct UpsertMcpServer<'a> {
    user_id: &'a str,
    name: &'a str,
    description: Option<&'a str>,
    transport: &'a McpServerTransport,
    original_json: Option<&'a str>,
    builtin: bool,
    enabled: bool,
}

impl McpConfigService {
    pub fn new(repo: Arc<dyn IMcpServerRepository>) -> Self {
        Self {
            repo,
            recent_team_sync: Arc::default(),
            selection_mutation: Arc::default(),
        }
    }

    /// List all MCP servers.
    pub async fn list_servers(&self, user_id: &str) -> Result<Vec<McpServerResponse>, McpError> {
        let _guard = self.selection_mutation.lock().await;
        self.list_servers_locked(user_id).await
    }

    // Legacy quarantine must serialize with explicit replacement and team sync:
    // a stale over-limit read must not clear a newly saved valid selection.
    async fn list_servers_locked(&self, user_id: &str) -> Result<Vec<McpServerResponse>, McpError> {
        let mut rows = self.repo.list(user_id).await?;
        if rows.iter().filter(|r| r.enabled && !r.builtin).count() > dream_core_common::mcp_policy::MAX_USER_MCP_SERVERS
        {
            self.repo.replace_enabled_selection(user_id, &[]).await?;
            warn!(
                user_id,
                "Historical oversized MCP activation quarantined; explicit selection required"
            );
            rows = self.repo.list(user_id).await?;
        }
        for row in &mut rows {
            if let Ok(server) = McpServer::from_row(row.clone())
                && let McpServerTransport::Stdio { command, args, .. } = &server.transport
                && dream_core_common::mcp_policy::is_browser_auth_proxy(command, args)
            {
                match normalize_transport(server.transport) {
                    Ok(native) => {
                        let json = native.to_config_json()?;
                        *row = self
                            .repo
                            .update(
                                user_id,
                                &row.id,
                                UpdateMcpServerParams {
                                    transport_type: Some(native.transport_type()),
                                    transport_config: Some(&json),
                                    ..Default::default()
                                },
                            )
                            .await?;
                        info!(server_id=%row.id, "Browser-authorizing MCP proxy migrated to native HTTP");
                    }
                    Err(_) => {
                        *row = self
                            .repo
                            .update(
                                user_id,
                                &row.id,
                                UpdateMcpServerParams {
                                    enabled: Some(false),
                                    ..Default::default()
                                },
                            )
                            .await?;
                        warn!(server_id=%row.id, "Unsupported browser-authorizing MCP proxy disabled; native HTTP configuration required");
                    }
                }
            }
        }
        rows.into_iter()
            .map(|row| McpServer::from_row(row).map(McpServer::into_response))
            .collect()
    }

    /// Get a single MCP server by ID.
    pub async fn get_server(&self, user_id: &str, id: &str) -> Result<McpServerResponse, McpError> {
        let row = self
            .repo
            .find_by_id(user_id, id)
            .await?
            .ok_or_else(|| McpError::NotFound(id.to_owned()))?;
        let server = McpServer::from_row(row)?;
        Ok(server.into_response())
    }

    /// Add (or upsert) an MCP server.
    ///
    /// If a server with the same name already exists, it is updated
    /// (transport, description, original_json) rather than creating a duplicate.
    pub async fn add_server(&self, user_id: &str, req: CreateMcpServerRequest) -> Result<McpServerResponse, McpError> {
        let transport = normalize_transport(McpServerTransport::from(req.transport))?;
        self.upsert_server(UpsertMcpServer {
            user_id,
            name: &req.name,
            description: req.description.as_deref(),
            transport: &transport,
            original_json: req.original_json.as_deref(),
            builtin: req.builtin,
            enabled: false,
        })
        .await
    }

    /// Edit an existing MCP server (partial update).
    pub async fn edit_server(
        &self,
        user_id: &str,
        id: &str,
        req: UpdateMcpServerRequest,
    ) -> Result<McpServerResponse, McpError> {
        // Verify the server exists
        let existing_server = self
            .repo
            .find_by_id(user_id, id)
            .await?
            .ok_or_else(|| McpError::NotFound(id.to_owned()))?;

        if let Some(ref new_name) = req.name
            && new_name != &existing_server.name
        {
            return Err(McpError::InvalidEdit(format!(
                "MCP server name cannot be changed during edit; keep '{current_name}'",
                current_name = existing_server.name
            )));
        }

        // Check name uniqueness if renaming
        if let Some(ref new_name) = req.name
            && let Some(existing) = self.repo.find_by_name_any(user_id, new_name).await?
            && existing.id != id
        {
            if existing.builtin {
                return Err(McpError::Conflict(format!(
                    "Builtin MCP server name '{new_name}' is reserved"
                )));
            }
            return Err(McpError::Conflict(new_name.clone()));
        }

        // Build transport fields if provided
        let transport = req
            .transport
            .map(McpServerTransport::from)
            .map(normalize_transport)
            .transpose()?;
        let config_json = transport.as_ref().map(McpServerTransport::to_config_json).transpose()?;

        let params = UpdateMcpServerParams {
            name: req.name.as_deref(),
            description: req.description.as_ref().map(|opt| opt.as_deref()),
            transport_type: transport.as_ref().map(McpServerTransport::transport_type),
            transport_config: config_json.as_deref(),
            original_json: req.original_json.as_ref().map(|opt| opt.as_deref()),
            builtin: req.builtin,
            ..Default::default()
        };

        let row = self.repo.update(user_id, id, params).await?;
        let server = McpServer::from_row(row)?;
        Ok(server.into_response())
    }

    /// Soft-delete an MCP server by ID.
    ///
    /// Returns whether the deleted server was enabled.
    pub async fn delete_server(&self, user_id: &str, id: &str) -> Result<bool, McpError> {
        let row = self
            .repo
            .find_by_id(user_id, id)
            .await?
            .ok_or_else(|| McpError::NotFound(id.to_owned()))?;
        let was_enabled = row.enabled;
        self.repo.delete(user_id, id).await?;
        Ok(was_enabled)
    }

    /// Toggle the enabled state of an MCP server.
    ///
    /// Returns the updated server response.
    pub async fn toggle_server(&self, user_id: &str, id: &str) -> Result<McpServerResponse, McpError> {
        let row = self
            .repo
            .find_by_id(user_id, id)
            .await?
            .ok_or_else(|| McpError::NotFound(id.to_owned()))?;

        let new_enabled = !row.enabled;
        if !row.builtin {
            let mut ids: Vec<_> = self
                .repo
                .list(user_id)
                .await?
                .into_iter()
                .filter(|r| r.enabled && !r.builtin && r.id != id)
                .map(|r| r.id)
                .collect();
            if new_enabled {
                ids.push(id.to_owned());
            }
            self.replace_enabled_selection(user_id, &ids).await?;
            return self.get_server(user_id, id).await;
        }
        let params = UpdateMcpServerParams {
            enabled: Some(new_enabled),
            ..Default::default()
        };
        let updated = self.repo.update(user_id, id, params).await?;
        let server = McpServer::from_row(updated)?;
        Ok(server.into_response())
    }

    pub async fn replace_enabled_selection(
        &self,
        user_id: &str,
        ids: &[String],
    ) -> Result<Vec<McpServerResponse>, McpError> {
        let _guard = self.selection_mutation.lock().await;
        if ids.iter().collect::<std::collections::HashSet<_>>().len()
            > dream_core_common::mcp_policy::MAX_USER_MCP_SERVERS
        {
            return Err(McpError::InvalidEdit(
                "MCP_LIMIT_EXCEEDED: remove an existing MCP before adding another (maximum 20)".into(),
            ));
        }
        self.repo.replace_enabled_selection(user_id, ids).await?;
        self.list_servers_locked(user_id).await
    }

    /// Batch import MCP servers (upsert by name).
    ///
    /// Each server is processed individually: existing names are updated,
    /// new names are created.
    pub async fn batch_import(
        &self,
        user_id: &str,
        req: BatchImportMcpServersRequest,
    ) -> Result<Vec<McpServerResponse>, McpError> {
        let requested_count = req.servers.len();
        let mut rows = Vec::with_capacity(requested_count);
        let mut skipped_reserved_count = 0usize;
        for server_req in req.servers {
            if let Some(existing) = self.repo.find_by_name_any(user_id, &server_req.name).await?
                && existing.builtin
            {
                skipped_reserved_count += 1;
                warn!(
                    name = %server_req.name,
                    "skipping batch import for builtin MCP name"
                );
                continue;
            }

            let transport = normalize_transport(McpServerTransport::from(server_req.transport))?;
            let server = self
                .upsert_server(UpsertMcpServer {
                    user_id,
                    name: &server_req.name,
                    description: server_req.description.as_deref(),
                    transport: &transport,
                    original_json: server_req.original_json.as_deref(),
                    builtin: server_req.builtin,
                    enabled: false,
                })
                .await?;
            rows.push(server);
        }
        info!(
            requested_count,
            imported_count = rows.len(),
            skipped_reserved_count,
            enabled_count = rows.iter().filter(|row| row.enabled).count(),
            "batch imported MCP servers"
        );
        Ok(rows)
    }

    /// Persist the latest connection test result for an existing MCP server.
    pub async fn persist_test_result(
        &self,
        user_id: &str,
        id: &str,
        result: &McpConnectionTestResult,
    ) -> Result<(), McpError> {
        let status = if result.success { "connected" } else { "error" };
        let last_connected = if result.success { Some(now_ms()) } else { None };
        let tools_json = result.tools.as_ref().map(serde_json::to_string).transpose()?;

        self.repo.update_status(user_id, id, status, last_connected).await?;
        self.repo.update_tools(user_id, id, tools_json.as_deref()).await?;
        Ok(())
    }

    async fn upsert_server(&self, params: UpsertMcpServer<'_>) -> Result<McpServerResponse, McpError> {
        let UpsertMcpServer {
            user_id,
            name,
            description,
            transport,
            original_json,
            builtin,
            enabled,
        } = params;
        let config_json = transport.to_config_json()?;

        if let Some(existing) = self.repo.find_by_name_any(user_id, name).await? {
            if existing.builtin {
                return Err(McpError::Conflict(format!(
                    "Builtin MCP server name '{name}' is reserved"
                )));
            }

            let params = UpdateMcpServerParams {
                description: Some(description),
                enabled: Some(enabled),
                transport_type: Some(transport.transport_type()),
                transport_config: Some(&config_json),
                original_json: Some(original_json),
                builtin: Some(existing.builtin || builtin),
                deleted_at: Some(None),
                ..Default::default()
            };
            let updated = self.repo.update(user_id, &existing.id, params).await?;
            let server = McpServer::from_row(updated)?;
            return Ok(server.into_response());
        }

        let params = CreateMcpServerParams {
            user_id,
            name,
            description,
            enabled,
            transport_type: transport.transport_type(),
            transport_config: &config_json,
            tools: None,
            original_json,
            builtin,
        };
        let row = self.repo.create(params).await?;
        let server = McpServer::from_row(row)?;
        Ok(server.into_response())
    }
}

pub(crate) fn normalize_transport(transport: McpServerTransport) -> Result<McpServerTransport, McpError> {
    if let McpServerTransport::Stdio { command, args, env } = &transport
        && dream_core_common::mcp_policy::is_browser_auth_proxy(command, args)
    {
        // Native HTTP never invokes an OS browser. Keep explicit header auth,
        // but reject proxy-specific OAuth options instead of silently dropping them.
        let index = args
            .iter()
            .position(|a| a.starts_with("https://") || a.starts_with("http://"))
            .ok_or_else(|| McpError::InvalidTransport("mcp-remote requires a native HTTP endpoint".into()))?;
        let url = args[index].clone();
        let mut headers = std::collections::HashMap::new();
        let mut i = index + 1;
        while i < args.len() {
            if args[i] == "--header" && i + 1 < args.len() {
                let (name, value) = args[i + 1]
                    .split_once(':')
                    .ok_or_else(|| McpError::InvalidTransport("Invalid remote MCP header".into()))?;
                let mut value = value.trim().to_owned();
                for (key, replacement) in env {
                    value = value.replace(&format!("${{{key}}}"), replacement);
                }
                if value.contains("${") {
                    return Err(McpError::InvalidTransport(
                        "Remote MCP header references an unavailable environment variable".into(),
                    ));
                }
                headers.insert(name.trim().to_owned(), value);
                i += 2;
            } else if args[i] == "--allow-http" || args[i].parse::<u16>().is_ok() {
                i += 1;
            } else {
                return Err(McpError::InvalidTransport("MCP_BROWSER_PROXY_DISABLED: configure the equivalent native HTTP transport and authorize explicitly".into()));
            }
        }
        return Ok(McpServerTransport::Http { url, headers });
    }
    match transport {
        McpServerTransport::Stdio { command, args, env } if args.is_empty() => {
            let Some((normalized_command, normalized_args)) = split_stdio_command(&command)? else {
                return Ok(McpServerTransport::Stdio { command, args, env });
            };
            normalize_transport(McpServerTransport::Stdio {
                command: normalized_command,
                args: normalized_args,
                env,
            })
        }
        _ => Ok(transport),
    }
}

fn split_stdio_command(command: &str) -> Result<Option<(String, Vec<String>)>, McpError> {
    let trimmed = command.trim();
    if trimmed.is_empty() || !trimmed.contains(char::is_whitespace) {
        return Ok(None);
    }

    let first_token = trimmed
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(&['"', '\''][..]);
    if !SPLITTABLE_STDIO_LAUNCHERS.contains(&first_token) {
        return Ok(None);
    }

    let tokens = shell_split(trimmed).map_err(McpError::InvalidTransport)?;
    if tokens.len() < 2 {
        return Ok(None);
    }

    Ok(Some((tokens[0].clone(), tokens[1..].to_vec())))
}

/// Split a shell-style command string into tokens, honoring quotes.
///
/// Backslash handling is **platform-dependent by design**. This splits
/// commands that will be spawned on the local OS, not passed to a POSIX
/// shell — and on Windows, `\` is the path separator, not an escape
/// character. Treating it as POSIX escaping silently ate every backslash:
/// `node D:\1one-command\out\main\x.js` became `node D:1one-commandoutmainx.js`,
/// a real bug that broke every Windows path with a launcher argument. On
/// Windows `\` is copied through literally; on Unix it keeps standard
/// escape semantics (`\` + next char → that char literally).
pub fn shell_split(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        match quote {
            Some(active) => {
                if ch == active {
                    quote = None;
                } else if ch == '\\' && active == '"' && !cfg!(windows) {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                '\\' if !cfg!(windows) => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(ch),
            },
        }
    }

    if quote.is_some() {
        return Err("Unterminated quoted command string".to_owned());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Team MCP distribution (M3) — materialize enterprise registry entries into
// the member's local MCP config so agents actually connect to them, offline.
// ---------------------------------------------------------------------------

/// JSON key stored in `original_json` marking a server as team-distributed.
/// Ownership marker for reconcile: only servers carrying this key are ever
/// touched by team sync — a member's personal servers are never clobbered.
const TEAM_REGISTRY_KEY: &str = "teamRegistryId";

/// One team MCP connector fetched from the enterprise registry.
#[derive(Debug, Clone)]
pub struct TeamMcpPayload {
    /// Stable registry id (`omcp_*`); reconcile key.
    pub registry_id: String,
    pub name: String,
    /// `stdio` (endpoint = command line) or `sse` (endpoint = URL).
    pub server_type: String,
    pub endpoint: String,
    pub enabled: bool,
    /// D5: distributed credentials — a JSON object materialized as stdio `env`
    /// or sse `headers` so the connector authenticates locally (offline).
    pub secrets_json: Option<String>,
    /// Bound by a distributed digital employee but not selected by the member.
    /// Stored disabled and outside the member's 20-connector quota; only an
    /// employee run resolves it (see [`McpConfigService::team_server_transport`]).
    pub employee_bound: bool,
}

/// Parse a `{ "k": "v" }` secrets blob into a string map. Non-string values
/// and malformed JSON yield an empty map (never fails the sync).
fn parse_secret_map(secrets_json: Option<&str>) -> std::collections::HashMap<String, String> {
    let Some(raw) = secrets_json else {
        return Default::default();
    };
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|obj| {
            obj.into_iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// Outcome of a team MCP sync pass.
#[derive(Debug, Clone, Default)]
pub struct TeamMcpSyncReport {
    pub written: Vec<String>,
    pub removed: Vec<String>,
    /// Names skipped because a personal (non-team) server already owns them.
    pub conflicts: Vec<String>,
    pub kept: usize,
}

const TEAM_EMPLOYEE_BOUND_KEY: &str = "teamEmployeeBound";

fn team_origin_json(registry_id: &str, employee_bound: bool) -> String {
    if employee_bound {
        serde_json::json!({ TEAM_REGISTRY_KEY: registry_id, TEAM_EMPLOYEE_BOUND_KEY: true }).to_string()
    } else {
        serde_json::json!({ TEAM_REGISTRY_KEY: registry_id }).to_string()
    }
}

fn team_employee_bound_of(original_json: Option<&str>) -> bool {
    original_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.get(TEAM_EMPLOYEE_BOUND_KEY)?.as_bool())
        .unwrap_or(false)
}

fn team_registry_id_of(original_json: Option<&str>) -> Option<String> {
    let raw = original_json?;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    value.get(TEAM_REGISTRY_KEY)?.as_str().map(str::to_owned)
}

impl McpConfigService {
    /// Local row id and transport of the enabled team connector this member's
    /// client materialized from registry row `registry_id`, if any.
    ///
    /// A digital employee distributed by the company names its connectors by
    /// registry id; on a member's machine the registry lives on the company
    /// server, and the only local copy is the one [`Self::sync_team_servers`]
    /// wrote — so that is where an employee run resolves its binding.
    pub async fn team_server_transport(
        &self,
        user_id: &str,
        registry_id: &str,
    ) -> Result<Option<(String, McpServerTransport)>, McpError> {
        for row in self.repo.list(user_id).await? {
            if team_registry_id_of(row.original_json.as_deref()).as_deref() != Some(registry_id) {
                continue;
            }
            // Employee-bound connectors are stored disabled on purpose; every
            // other team row must still be enabled to resolve.
            if !row.enabled && !team_employee_bound_of(row.original_json.as_deref()) {
                continue;
            }
            let server = McpServer::from_row(row)?;
            return Ok(Some((server.id, server.transport)));
        }
        Ok(None)
    }

    /// Materialize team registry MCP connectors into the local config and
    /// reconcile removals. `authoritative` MUST be true only when `payloads`
    /// is the complete current server view (server reachable); offline calls
    /// pass false so the local cache is never wiped.
    pub async fn sync_team_servers(
        &self,
        user_id: &str,
        payloads: &[TeamMcpPayload],
        authoritative: bool,
    ) -> Result<TeamMcpSyncReport, McpError> {
        let _guard = self.selection_mutation.lock().await;
        // Two separate buckets: what the member selected (their 20) and what
        // distributed employees bind (admin-governed, own 20-entry ceiling).
        let bound_count = payloads.iter().filter(|p| p.employee_bound).count();
        if payloads.len() - bound_count > dream_core_common::mcp_policy::MAX_USER_MCP_SERVERS {
            return Err(McpError::InvalidEdit(
                "MCP_LIMIT_EXCEEDED: enterprise distribution must contain at most 20 explicitly selected connectors"
                    .into(),
            ));
        }
        if bound_count > dream_core_common::mcp_policy::MAX_USER_MCP_SERVERS {
            return Err(McpError::InvalidEdit(
                "MCP_LIMIT_EXCEEDED: digital employees may bind at most 20 connectors per member".into(),
            ));
        }
        let _ = self.list_servers_locked(user_id).await?;
        let personal_count = self
            .repo
            .list(user_id)
            .await?
            .iter()
            .filter(|r| r.enabled && !r.builtin && team_registry_id_of(r.original_json.as_deref()).is_none())
            .count();
        if personal_count + payloads.iter().filter(|p| p.enabled && !p.employee_bound).count()
            > dream_core_common::mcp_policy::MAX_USER_MCP_SERVERS
        {
            return Err(McpError::InvalidEdit(
                "MCP_LIMIT_EXCEEDED: replace personal connectors before enabling company connectors".into(),
            ));
        }
        let digest = team_sync_digest(payloads, authoritative);
        if let Ok(recent) = self.recent_team_sync.lock()
            && let Some(last) = recent.get(user_id)
            && last.digest == digest
            && last.at.elapsed() < TEAM_SYNC_MEMO_TTL
        {
            return Ok(last.report.clone());
        }
        let report = self
            .sync_team_servers_uncached(user_id, payloads, authoritative)
            .await?;
        tracing::info!(
            user_id,
            authoritative,
            requested_count = payloads.len(),
            written_count = report.written.len(),
            removed_count = report.removed.len(),
            conflict_count = report.conflicts.len(),
            reason = "enterprise_selection_sync",
            "Reconciled selected enterprise MCP connectors"
        );
        if let Ok(mut recent) = self.recent_team_sync.lock() {
            recent.insert(
                user_id.to_owned(),
                RecentTeamSync {
                    digest,
                    at: std::time::Instant::now(),
                    report: report.clone(),
                },
            );
        }
        Ok(report)
    }

    async fn sync_team_servers_uncached(
        &self,
        user_id: &str,
        payloads: &[TeamMcpPayload],
        authoritative: bool,
    ) -> Result<TeamMcpSyncReport, McpError> {
        let mut report = TeamMcpSyncReport::default();
        let mut wanted: std::collections::HashSet<String> = std::collections::HashSet::new();

        for payload in payloads {
            let secrets = parse_secret_map(payload.secrets_json.as_deref());
            let transport = match payload.server_type.as_str() {
                "stdio" => {
                    let tokens = shell_split(&payload.endpoint)
                        .map_err(|e| McpError::InvalidTransport(format!("team stdio endpoint: {e}")))?;
                    let Some((command, args)) = tokens.split_first() else {
                        report.conflicts.push(payload.name.clone());
                        continue;
                    };
                    McpServerTransport::Stdio {
                        command: command.clone(),
                        args: args.to_vec(),
                        env: secrets,
                    }
                }
                _ => McpServerTransport::Sse {
                    url: payload.endpoint.clone(),
                    headers: secrets,
                },
            };

            let transport = normalize_transport(transport)?;

            // Conflict guard: never clobber a member's personal server that
            // happens to share the name — only servers we own (team marker).
            let existing = self.repo.find_by_name_any(user_id, &payload.name).await?;
            if let Some(existing) = &existing
                && team_registry_id_of(existing.original_json.as_deref()).is_none()
            {
                report.conflicts.push(payload.name.clone());
                continue;
            }

            let description = "Team-distributed MCP connector";
            let original_json = team_origin_json(&payload.registry_id, payload.employee_bound);
            // An employee-bound connector never joins the member's active set.
            let enabled = payload.enabled && !payload.employee_bound;
            let config_json = transport.to_config_json()?;

            // Steady-state short circuit: this loop reruns every 5 minutes
            // for every server visible to this member — with the registry at
            // the size this org has, that is thousands of sequential
            // find-then-upsert DB round trips a cycle for connectors nothing
            // changed about. Skip the write when the row already matches.
            let unchanged = existing.as_ref().is_some_and(|row| {
                row.deleted_at.is_none()
                    && row.enabled == enabled
                    && row.description.as_deref() == Some(description)
                    && row.transport_type == transport.transport_type()
                    && row.transport_config == config_json
                    && row.original_json.as_deref() == Some(original_json.as_str())
            });

            if !unchanged {
                self.upsert_server(UpsertMcpServer {
                    user_id,
                    name: &payload.name,
                    description: Some(description),
                    transport: &transport,
                    original_json: Some(&original_json),
                    builtin: false,
                    enabled,
                })
                .await?;
            }
            wanted.insert(payload.registry_id.clone());
            report.written.push(payload.name.clone());
        }

        if authoritative {
            for row in self.repo.list(user_id).await? {
                if let Some(registry_id) = team_registry_id_of(row.original_json.as_deref())
                    && !wanted.contains(&registry_id)
                {
                    self.repo.delete(user_id, &row.id).await?;
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_api_types::{ImportMcpServerRequest, McpTransport};
    use dream_core_common::{McpServerStatus, TimestampMs};
    use dream_core_db::models::McpServerRow;
    use dream_core_db::{CreateMcpServerParams, DbError, UpdateMcpServerParams};
    use std::collections::HashMap;
    use std::sync::Mutex;

    const TEST_USER_ID: &str = "user-1";

    // -- In-memory mock repository -------------------------------------------

    #[derive(Debug)]
    pub(super) struct MockMcpServerRepo {
        servers: Mutex<Vec<McpServerRow>>,
        id_counter: Mutex<u32>,
        /// Write-call counters, so a test can assert a resync that changed
        /// nothing skipped the DB write entirely rather than inferring it
        /// from `updated_at` (the mock clock is fixed, so that would prove
        /// nothing).
        create_calls: Mutex<u32>,
        update_calls: Mutex<u32>,
    }

    impl MockMcpServerRepo {
        pub(super) fn new() -> Self {
            Self {
                servers: Mutex::new(Vec::new()),
                id_counter: Mutex::new(0),
                create_calls: Mutex::new(0),
                update_calls: Mutex::new(0),
            }
        }

        pub(super) fn write_call_count(&self) -> u32 {
            *self.create_calls.lock().unwrap() + *self.update_calls.lock().unwrap()
        }

        fn next_id(&self) -> String {
            let mut counter = self.id_counter.lock().unwrap();
            *counter += 1;
            format!("mcp_{counter}")
        }

        fn now() -> TimestampMs {
            1000
        }
    }

    #[async_trait::async_trait]
    impl IMcpServerRepository for MockMcpServerRepo {
        async fn replace_enabled_selection(&self, user_id: &str, ids: &[String]) -> Result<(), DbError> {
            if ids.len() > dream_core_common::mcp_policy::MAX_USER_MCP_SERVERS {
                return Err(DbError::Conflict("MCP_LIMIT_EXCEEDED".into()));
            }
            let mut rows = self.servers.lock().unwrap();
            if ids.iter().any(|id| {
                !rows
                    .iter()
                    .any(|r| r.user_id == user_id && &r.id == id && !r.builtin && r.deleted_at.is_none())
            }) {
                return Err(DbError::NotFound("MCP selection".into()));
            }
            for row in rows
                .iter_mut()
                .filter(|r| r.user_id == user_id && !r.builtin && r.deleted_at.is_none())
            {
                row.enabled = ids.contains(&row.id);
            }
            Ok(())
        }

        async fn list(&self, user_id: &str) -> Result<Vec<McpServerRow>, DbError> {
            let servers = self.servers.lock().unwrap();
            Ok(servers
                .iter()
                .filter(|s| s.user_id == user_id && s.deleted_at.is_none())
                .cloned()
                .collect())
        }

        async fn find_by_id(&self, user_id: &str, id: &str) -> Result<Option<McpServerRow>, DbError> {
            let servers = self.servers.lock().unwrap();
            Ok(servers
                .iter()
                .find(|s| s.user_id == user_id && s.id == id && s.deleted_at.is_none())
                .cloned())
        }

        async fn find_by_name(&self, user_id: &str, name: &str) -> Result<Option<McpServerRow>, DbError> {
            let servers = self.servers.lock().unwrap();
            Ok(servers
                .iter()
                .find(|s| s.user_id == user_id && s.name == name && s.deleted_at.is_none())
                .cloned())
        }

        async fn find_by_id_any(&self, user_id: &str, id: &str) -> Result<Option<McpServerRow>, DbError> {
            let servers = self.servers.lock().unwrap();
            Ok(servers.iter().find(|s| s.user_id == user_id && s.id == id).cloned())
        }

        async fn find_by_name_any(&self, user_id: &str, name: &str) -> Result<Option<McpServerRow>, DbError> {
            let servers = self.servers.lock().unwrap();
            Ok(servers.iter().find(|s| s.user_id == user_id && s.name == name).cloned())
        }

        async fn list_by_ids_any(&self, user_id: &str, ids: &[String]) -> Result<Vec<McpServerRow>, DbError> {
            let servers = self.servers.lock().unwrap();
            Ok(servers
                .iter()
                .filter(|server| server.user_id == user_id && ids.iter().any(|id| id == &server.id))
                .cloned()
                .collect())
        }

        async fn create(&self, params: CreateMcpServerParams<'_>) -> Result<McpServerRow, DbError> {
            *self.create_calls.lock().unwrap() += 1;
            let mut servers = self.servers.lock().unwrap();
            if servers.iter().any(|s| s.name == params.name) {
                return Err(DbError::Conflict(format!(
                    "MCP server name '{}' already exists",
                    params.name
                )));
            }
            let row = McpServerRow {
                id: self.next_id(),
                user_id: params.user_id.to_owned(),
                name: params.name.to_owned(),
                description: params.description.map(String::from),
                enabled: params.enabled,
                transport_type: params.transport_type.to_owned(),
                transport_config: params.transport_config.to_owned(),
                tools: params.tools.map(String::from),
                last_test_status: "disconnected".to_owned(),
                last_connected: None,
                original_json: params.original_json.map(String::from),
                builtin: params.builtin,
                deleted_at: None,
                created_at: Self::now(),
                updated_at: Self::now(),
            };
            servers.push(row.clone());
            Ok(row)
        }

        async fn update(
            &self,
            user_id: &str,
            id: &str,
            params: UpdateMcpServerParams<'_>,
        ) -> Result<McpServerRow, DbError> {
            *self.update_calls.lock().unwrap() += 1;
            let mut servers = self.servers.lock().unwrap();
            let idx = servers
                .iter()
                .position(|s| s.user_id == user_id && s.id == id)
                .ok_or_else(|| DbError::NotFound(format!("MCP server {id}")))?;

            // Check name conflict
            if let Some(new_name) = params.name {
                if servers
                    .iter()
                    .any(|s| s.user_id == user_id && s.name == new_name && s.id != id)
                {
                    return Err(DbError::Conflict(format!(
                        "MCP server name '{new_name}' already exists"
                    )));
                }
                servers[idx].name = new_name.to_owned();
            }
            if let Some(desc) = params.description {
                servers[idx].description = desc.map(String::from);
            }
            if let Some(enabled) = params.enabled {
                servers[idx].enabled = enabled;
            }
            if let Some(tt) = params.transport_type {
                servers[idx].transport_type = tt.to_owned();
            }
            if let Some(tc) = params.transport_config {
                servers[idx].transport_config = tc.to_owned();
            }
            if let Some(tools) = params.tools {
                servers[idx].tools = tools.map(String::from);
            }
            if let Some(oj) = params.original_json {
                servers[idx].original_json = oj.map(String::from);
            }
            if let Some(b) = params.builtin {
                servers[idx].builtin = b;
            }
            if let Some(deleted_at) = params.deleted_at {
                servers[idx].deleted_at = deleted_at;
            }
            servers[idx].updated_at = Self::now();
            Ok(servers[idx].clone())
        }

        async fn delete(&self, user_id: &str, id: &str) -> Result<(), DbError> {
            let mut servers = self.servers.lock().unwrap();
            let idx = servers
                .iter()
                .position(|s| s.user_id == user_id && s.id == id && s.deleted_at.is_none())
                .ok_or_else(|| DbError::NotFound(format!("MCP server {id}")))?;
            servers[idx].enabled = false;
            servers[idx].deleted_at = Some(Self::now());
            servers[idx].updated_at = Self::now();
            Ok(())
        }

        async fn batch_upsert(
            &self,
            user_id: &str,
            params_list: &[CreateMcpServerParams<'_>],
        ) -> Result<Vec<McpServerRow>, DbError> {
            let mut results = Vec::new();
            for params in params_list {
                let mut servers = self.servers.lock().unwrap();
                if let Some(idx) = servers
                    .iter()
                    .position(|s| s.user_id == user_id && s.name == params.name)
                {
                    // Update existing
                    servers[idx].description = params.description.map(String::from);
                    servers[idx].transport_type = params.transport_type.to_owned();
                    servers[idx].transport_config = params.transport_config.to_owned();
                    servers[idx].original_json = params.original_json.map(String::from);
                    servers[idx].updated_at = Self::now();
                    results.push(servers[idx].clone());
                } else {
                    // Create new
                    let row = McpServerRow {
                        id: self.next_id(),
                        user_id: params.user_id.to_owned(),
                        name: params.name.to_owned(),
                        description: params.description.map(String::from),
                        enabled: params.enabled,
                        transport_type: params.transport_type.to_owned(),
                        transport_config: params.transport_config.to_owned(),
                        tools: params.tools.map(String::from),
                        last_test_status: "disconnected".to_owned(),
                        last_connected: None,
                        original_json: params.original_json.map(String::from),
                        builtin: params.builtin,
                        deleted_at: None,
                        created_at: Self::now(),
                        updated_at: Self::now(),
                    };
                    servers.push(row.clone());
                    results.push(row);
                }
            }
            Ok(results)
        }

        async fn update_status(
            &self,
            user_id: &str,
            id: &str,
            status: &str,
            last_connected: Option<TimestampMs>,
        ) -> Result<(), DbError> {
            let mut servers = self.servers.lock().unwrap();
            let idx = servers
                .iter()
                .position(|s| s.user_id == user_id && s.id == id)
                .ok_or_else(|| DbError::NotFound(format!("MCP server {id}")))?;
            servers[idx].last_test_status = status.to_owned();
            if let Some(lc) = last_connected {
                servers[idx].last_connected = Some(lc);
            }
            Ok(())
        }

        async fn update_tools(&self, user_id: &str, id: &str, tools: Option<&str>) -> Result<(), DbError> {
            let mut servers = self.servers.lock().unwrap();
            let idx = servers
                .iter()
                .position(|s| s.user_id == user_id && s.id == id)
                .ok_or_else(|| DbError::NotFound(format!("MCP server {id}")))?;
            servers[idx].tools = tools.map(String::from);
            Ok(())
        }
    }

    fn make_service() -> McpConfigService {
        McpConfigService::new(Arc::new(MockMcpServerRepo::new()))
    }

    fn stdio_create_req(name: &str) -> CreateMcpServerRequest {
        CreateMcpServerRequest {
            name: name.to_owned(),
            description: Some("test server".to_owned()),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@test/server".into()],
                env: HashMap::new(),
            },
            original_json: None,
            builtin: false,
        }
    }

    fn http_create_req(name: &str) -> CreateMcpServerRequest {
        CreateMcpServerRequest {
            name: name.to_owned(),
            description: None,
            transport: McpTransport::Http {
                url: "https://example.com/mcp".into(),
                headers: HashMap::new(),
            },
            original_json: None,
            builtin: false,
        }
    }

    fn stdio_import_req(name: &str) -> ImportMcpServerRequest {
        ImportMcpServerRequest {
            name: name.to_owned(),
            description: Some("test server".to_owned()),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@test/server".into()],
                env: HashMap::new(),
            },
            original_json: None,
            builtin: false,
            enabled: None,
        }
    }

    fn http_import_req(name: &str) -> ImportMcpServerRequest {
        ImportMcpServerRequest {
            name: name.to_owned(),
            description: None,
            transport: McpTransport::Http {
                url: "https://example.com/mcp".into(),
                headers: HashMap::new(),
            },
            original_json: None,
            builtin: false,
            enabled: None,
        }
    }

    // -- list_servers --------------------------------------------------------

    #[tokio::test]
    async fn list_servers_empty() {
        let svc = make_service();
        let result = svc.list_servers(TEST_USER_ID).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn list_servers_returns_all() {
        let svc = make_service();
        svc.add_server(TEST_USER_ID, stdio_create_req("a")).await.unwrap();
        svc.add_server(TEST_USER_ID, http_create_req("b")).await.unwrap();

        let result = svc.list_servers(TEST_USER_ID).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    // -- get_server ----------------------------------------------------------

    #[tokio::test]
    async fn get_server_found() {
        let svc = make_service();
        let created = svc.add_server(TEST_USER_ID, stdio_create_req("test")).await.unwrap();
        let found = svc.get_server(TEST_USER_ID, &created.id).await.unwrap();
        assert_eq!(found.id, created.id);
        assert_eq!(found.name, "test");
    }

    #[tokio::test]
    async fn get_server_not_found() {
        let svc = make_service();
        let result = svc.get_server(TEST_USER_ID, "nonexistent").await;
        assert!(matches!(result, Err(McpError::NotFound(_))));
    }

    // -- add_server ----------------------------------------------------------

    #[tokio::test]
    async fn add_server_creates_new() {
        let svc = make_service();
        let resp = svc.add_server(TEST_USER_ID, stdio_create_req("new-srv")).await.unwrap();
        assert_eq!(resp.name, "new-srv");
        assert!(!resp.enabled);
        assert_eq!(resp.last_test_status, McpServerStatus::Disconnected);
        assert_eq!(resp.description.as_deref(), Some("test server"));
    }

    #[tokio::test]
    async fn add_server_upserts_existing() {
        let svc = make_service();
        let first = svc
            .add_server(TEST_USER_ID, stdio_create_req("upsert-test"))
            .await
            .unwrap();

        // Second add with same name updates existing
        let updated = svc
            .add_server(TEST_USER_ID, http_create_req("upsert-test"))
            .await
            .unwrap();
        assert_eq!(updated.id, first.id);
        // Transport should be updated to http
        match updated.transport {
            McpTransport::Http { ref url, .. } => {
                assert_eq!(url, "https://example.com/mcp");
            }
            _ => panic!("expected Http transport after upsert"),
        }
    }

    #[tokio::test]
    async fn add_server_stdio_complete() {
        let svc = make_service();
        let resp = svc
            .add_server(
                TEST_USER_ID,
                CreateMcpServerRequest {
                    name: "stdio-full".into(),
                    description: Some("full stdio".into()),
                    transport: McpTransport::Stdio {
                        command: "node".into(),
                        args: vec!["index.js".into()],
                        env: HashMap::from([("KEY".into(), "val".into())]),
                    },
                    original_json: Some(r#"{"name":"stdio-full"}"#.into()),
                    builtin: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.name, "stdio-full");
        assert!(resp.builtin);
        assert_eq!(resp.original_json.as_deref(), Some(r#"{"name":"stdio-full"}"#));
    }

    // -- edit_server ---------------------------------------------------------

    #[tokio::test]
    async fn edit_server_rejects_name_change() {
        let svc = make_service();
        let created = svc
            .add_server(TEST_USER_ID, stdio_create_req("old-name"))
            .await
            .unwrap();
        let err = svc
            .edit_server(
                TEST_USER_ID,
                &created.id,
                UpdateMcpServerRequest {
                    name: Some("new-name".into()),
                    description: None,
                    transport: None,
                    original_json: None,
                    builtin: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::InvalidEdit(_)));
    }

    #[tokio::test]
    async fn edit_server_updates_transport() {
        let svc = make_service();
        let created = svc.add_server(TEST_USER_ID, stdio_create_req("test")).await.unwrap();
        let updated = svc
            .edit_server(
                TEST_USER_ID,
                &created.id,
                UpdateMcpServerRequest {
                    name: None,
                    description: None,
                    transport: Some(McpTransport::Http {
                        url: "https://new.url".into(),
                        headers: HashMap::new(),
                    }),
                    original_json: None,
                    builtin: None,
                },
            )
            .await
            .unwrap();
        match updated.transport {
            McpTransport::Http { ref url, .. } => assert_eq!(url, "https://new.url"),
            _ => panic!("expected Http"),
        }
    }

    #[tokio::test]
    async fn edit_server_clears_description() {
        let svc = make_service();
        let created = svc.add_server(TEST_USER_ID, stdio_create_req("test")).await.unwrap();
        assert!(created.description.is_some());

        let updated = svc
            .edit_server(
                TEST_USER_ID,
                &created.id,
                UpdateMcpServerRequest {
                    name: None,
                    description: Some(None), // clear
                    transport: None,
                    original_json: None,
                    builtin: None,
                },
            )
            .await
            .unwrap();
        assert!(updated.description.is_none());
    }

    #[tokio::test]
    async fn edit_server_not_found() {
        let svc = make_service();
        let result = svc
            .edit_server(
                TEST_USER_ID,
                "nonexistent",
                UpdateMcpServerRequest {
                    name: Some("x".into()),
                    description: None,
                    transport: None,
                    original_json: None,
                    builtin: None,
                },
            )
            .await;
        assert!(matches!(result, Err(McpError::NotFound(_))));
    }

    #[tokio::test]
    async fn edit_server_name_conflict() {
        let svc = make_service();
        svc.add_server(TEST_USER_ID, stdio_create_req("server-a"))
            .await
            .unwrap();
        let b = svc
            .add_server(TEST_USER_ID, stdio_create_req("server-b"))
            .await
            .unwrap();

        let result = svc
            .edit_server(
                TEST_USER_ID,
                &b.id,
                UpdateMcpServerRequest {
                    name: Some("server-a".into()), // conflict
                    description: None,
                    transport: None,
                    original_json: None,
                    builtin: None,
                },
            )
            .await;
        assert!(matches!(result, Err(McpError::InvalidEdit(_))));
    }

    #[tokio::test]
    async fn edit_server_rename_to_same_name() {
        let svc = make_service();
        let a = svc
            .add_server(TEST_USER_ID, stdio_create_req("server-a"))
            .await
            .unwrap();

        // Renaming to the same name should succeed
        let result = svc
            .edit_server(
                TEST_USER_ID,
                &a.id,
                UpdateMcpServerRequest {
                    name: Some("server-a".into()),
                    description: None,
                    transport: None,
                    original_json: None,
                    builtin: None,
                },
            )
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn edit_server_updates_builtin_flag() {
        let svc = make_service();
        let created = svc
            .add_server(TEST_USER_ID, stdio_create_req("chrome-devtools"))
            .await
            .unwrap();
        assert!(!created.builtin);

        let updated = svc
            .edit_server(
                TEST_USER_ID,
                &created.id,
                UpdateMcpServerRequest {
                    name: None,
                    description: None,
                    transport: None,
                    original_json: None,
                    builtin: Some(true),
                },
            )
            .await
            .unwrap();
        assert!(updated.builtin);
    }

    // -- delete_server -------------------------------------------------------

    #[tokio::test]
    async fn delete_server_removes_and_returns_enabled_status() {
        let svc = make_service();
        let created = svc.add_server(TEST_USER_ID, stdio_create_req("test")).await.unwrap();

        // Not enabled
        let was_enabled = svc.delete_server(TEST_USER_ID, &created.id).await.unwrap();
        assert!(!was_enabled);

        // Should be hidden from active queries
        let result = svc.get_server(TEST_USER_ID, &created.id).await;
        assert!(matches!(result, Err(McpError::NotFound(_))));
    }

    #[tokio::test]
    async fn delete_enabled_server_returns_true() {
        let svc = make_service();
        let created = svc.add_server(TEST_USER_ID, stdio_create_req("test")).await.unwrap();
        svc.toggle_server(TEST_USER_ID, &created.id).await.unwrap(); // enable

        let was_enabled = svc.delete_server(TEST_USER_ID, &created.id).await.unwrap();
        assert!(was_enabled);
    }

    #[tokio::test]
    async fn delete_server_not_found() {
        let svc = make_service();
        let result = svc.delete_server(TEST_USER_ID, "nonexistent").await;
        assert!(matches!(result, Err(McpError::NotFound(_))));
    }

    // -- toggle_server -------------------------------------------------------

    #[tokio::test]
    async fn toggle_server_enables_then_disables() {
        let svc = make_service();
        let created = svc.add_server(TEST_USER_ID, stdio_create_req("toggle")).await.unwrap();
        assert!(!created.enabled);

        let toggled = svc.toggle_server(TEST_USER_ID, &created.id).await.unwrap();
        assert!(toggled.enabled);

        let toggled_back = svc.toggle_server(TEST_USER_ID, &created.id).await.unwrap();
        assert!(!toggled_back.enabled);
    }

    #[tokio::test]
    async fn toggle_server_not_found() {
        let svc = make_service();
        let result = svc.toggle_server(TEST_USER_ID, "nonexistent").await;
        assert!(matches!(result, Err(McpError::NotFound(_))));
    }

    // -- batch_import --------------------------------------------------------

    #[tokio::test]
    async fn batch_import_creates_new_servers() {
        let svc = make_service();
        let req = BatchImportMcpServersRequest {
            servers: vec![stdio_import_req("a"), http_import_req("b")],
        };
        let results = svc.batch_import(TEST_USER_ID, req).await.unwrap();
        assert_eq!(results.len(), 2);

        let all = svc.list_servers(TEST_USER_ID).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn batch_import_upserts_existing() {
        let svc = make_service();
        svc.add_server(TEST_USER_ID, stdio_create_req("existing"))
            .await
            .unwrap();

        let req = BatchImportMcpServersRequest {
            servers: vec![
                http_import_req("existing"),   // update
                stdio_import_req("brand-new"), // create
            ],
        };
        let results = svc.batch_import(TEST_USER_ID, req).await.unwrap();
        assert_eq!(results.len(), 2);

        let all = svc.list_servers(TEST_USER_ID).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn add_server_restores_soft_deleted_row() {
        let svc = make_service();
        let created = svc
            .add_server(TEST_USER_ID, stdio_create_req("restored"))
            .await
            .unwrap();
        svc.delete_server(TEST_USER_ID, &created.id).await.unwrap();

        let restored = svc.add_server(TEST_USER_ID, http_create_req("restored")).await.unwrap();
        assert_eq!(restored.id, created.id);
        match restored.transport {
            McpTransport::Http { .. } => {}
            _ => panic!("expected Http after restore"),
        }
    }

    #[tokio::test]
    async fn add_server_rejects_overriding_builtin_name() {
        let svc = make_service();
        svc.add_server(
            TEST_USER_ID,
            CreateMcpServerRequest {
                name: "chrome-devtools".into(),
                description: Some("builtin".into()),
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["-y".into(), "chrome-devtools-mcp@latest".into()],
                    env: HashMap::new(),
                },
                original_json: None,
                builtin: true,
            },
        )
        .await
        .unwrap();

        let err = svc
            .add_server(TEST_USER_ID, stdio_create_req("chrome-devtools"))
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::Conflict(_)));
    }

    #[tokio::test]
    async fn add_server_rejects_overriding_builtin_name_even_with_builtin_payload() {
        let svc = make_service();
        svc.add_server(
            TEST_USER_ID,
            CreateMcpServerRequest {
                name: "chrome-devtools".into(),
                description: Some("builtin".into()),
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["-y".into(), "chrome-devtools-mcp@latest".into()],
                    env: HashMap::new(),
                },
                original_json: None,
                builtin: true,
            },
        )
        .await
        .unwrap();

        let err = svc
            .add_server(
                TEST_USER_ID,
                CreateMcpServerRequest {
                    name: "chrome-devtools".into(),
                    description: Some("malicious override".into()),
                    transport: McpTransport::Http {
                        url: "https://example.com/mcp".into(),
                        headers: HashMap::new(),
                    },
                    original_json: None,
                    builtin: true,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::Conflict(_)));
    }

    #[tokio::test]
    async fn batch_import_skips_reserved_builtin_name() {
        let svc = make_service();
        svc.add_server(
            TEST_USER_ID,
            CreateMcpServerRequest {
                name: "chrome-devtools".into(),
                description: Some("builtin".into()),
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["-y".into(), "chrome-devtools-mcp@latest".into()],
                    env: HashMap::new(),
                },
                original_json: None,
                builtin: true,
            },
        )
        .await
        .unwrap();

        let results = svc
            .batch_import(
                TEST_USER_ID,
                BatchImportMcpServersRequest {
                    servers: vec![
                        ImportMcpServerRequest {
                            name: "chrome-devtools".into(),
                            description: Some("imported".into()),
                            transport: McpTransport::Http {
                                url: "https://example.com/mcp".into(),
                                headers: HashMap::new(),
                            },
                            original_json: None,
                            builtin: false,
                            enabled: Some(false),
                        },
                        ImportMcpServerRequest {
                            name: "playwright".into(),
                            description: Some("imported".into()),
                            transport: McpTransport::Stdio {
                                command: "npx".into(),
                                args: vec!["@playwright/mcp@latest".into()],
                                env: HashMap::new(),
                            },
                            original_json: None,
                            builtin: false,
                            enabled: Some(false),
                        },
                    ],
                },
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "playwright");
    }

    #[tokio::test]
    async fn add_server_normalizes_shell_style_stdio_command() {
        let svc = make_service();
        let created = svc
            .add_server(
                TEST_USER_ID,
                CreateMcpServerRequest {
                    name: "sentry".into(),
                    description: None,
                    transport: McpTransport::Stdio {
                        command: "npx @sentry/mcp-server@latest --organization-slug=demo".into(),
                        args: vec![],
                        env: HashMap::new(),
                    },
                    original_json: None,
                    builtin: false,
                },
            )
            .await
            .unwrap();

        match created.transport {
            McpTransport::Stdio { command, args, .. } => {
                assert_eq!(command, "npx");
                assert_eq!(args, vec!["@sentry/mcp-server@latest", "--organization-slug=demo"]);
            }
            _ => panic!("expected stdio transport"),
        }
    }

    // -- shell_split: platform-dependent backslash handling -------------------
    //
    // These exercise `shell_split` directly rather than through
    // `add_server`/`normalize_transport`, because `split_stdio_command` only
    // ever calls it for a fixed launcher whitelist (`node`, `npx`, ...) — the
    // backslash bug itself is launcher-agnostic and belongs at this level.

    #[test]
    #[cfg(windows)]
    fn shell_split_preserves_windows_path_backslashes() {
        // The exact regression: this used to come back as
        // ["node", "D:1one-commandoutmainbuiltin-mcp-web-tools.js"].
        let tokens = shell_split(r"node D:\1one-command\out\main\builtin-mcp-web-tools.js").unwrap();
        assert_eq!(
            tokens,
            vec!["node", r"D:\1one-command\out\main\builtin-mcp-web-tools.js"]
        );
    }

    #[test]
    #[cfg(windows)]
    fn shell_split_preserves_backslashes_inside_quotes_on_windows() {
        let tokens = shell_split(r#"node "D:\path with spaces\x.js""#).unwrap();
        assert_eq!(tokens, vec!["node", r"D:\path with spaces\x.js"]);
    }

    #[test]
    #[cfg(not(windows))]
    fn shell_split_treats_backslash_as_posix_escape_on_unix() {
        // Unquoted backslash escapes the next character (classic shell
        // behavior for escaping a space).
        let tokens = shell_split(r"node /opt/my\ tool/x.js").unwrap();
        assert_eq!(tokens, vec!["node", "/opt/my tool/x.js"]);
    }

    #[test]
    #[cfg(not(windows))]
    fn shell_split_posix_escape_inside_double_quotes() {
        let tokens = shell_split(r#"node "/opt/say \"hi\"/x.js""#).unwrap();
        assert_eq!(tokens, vec!["node", "/opt/say \"hi\"/x.js"]);
    }

    #[test]
    fn shell_split_still_honors_quoted_spaces_on_every_platform() {
        // Quoting behavior (not backslash-specific) must be unaffected by
        // the platform-dependent branch.
        //
        // The path deliberately carries no backslash. It used to, which made
        // a test whose whole point is platform-INdependence assert the one
        // thing that is platform-dependent — so it passed on Windows and
        // failed on Linux with the backslashes eaten. Quoted spaces are what
        // this pins; the backslash halves are pinned by the four cfg-gated
        // tests above.
        let tokens = shell_split(r#"node "/opt/Program Files/App/app.js" --flag"#).unwrap();
        assert_eq!(tokens, vec!["node", "/opt/Program Files/App/app.js", "--flag"]);
    }

    /// Windows-only, like the behaviour it pins — see `shell_split`.
    #[test]
    #[cfg(windows)]
    fn split_stdio_command_recovers_windows_node_path() {
        let result = split_stdio_command(r"node D:\1one-command\out\main\builtin-mcp-web-tools.js").unwrap();
        let (command, args) = result.expect("expected a split result for a whitelisted launcher");
        assert_eq!(command, "node");
        assert_eq!(args, vec![r"D:\1one-command\out\main\builtin-mcp-web-tools.js"]);
    }

    /// The same recovery on Unix, where the launcher split has to survive a
    /// POSIX-escaped space rather than a drive path.
    #[test]
    #[cfg(not(windows))]
    fn split_stdio_command_recovers_unix_node_path() {
        let result = split_stdio_command(r"node /opt/1one\ command/out/main/builtin-mcp-web-tools.js").unwrap();
        let (command, args) = result.expect("expected a split result for a whitelisted launcher");
        assert_eq!(command, "node");
        assert_eq!(args, vec!["/opt/1one command/out/main/builtin-mcp-web-tools.js"]);
    }

    /// Windows-only by construction, like the behaviour it pins.
    ///
    /// `shell_split` treats `\` as a path separator on Windows and as POSIX
    /// escaping everywhere else — a deliberate split documented on the function
    /// itself, because these commands are spawned on the local OS rather than
    /// handed to a POSIX shell. An ungated assertion of the Windows half
    /// therefore passed on the author's machine and failed the Linux CI job
    /// with the backslashes eaten, which is the very symptom the test is named
    /// after. The Unix half is asserted separately below.
    #[cfg(windows)]
    #[tokio::test]
    async fn add_server_normalizes_windows_node_path_without_losing_backslashes() {
        // End-to-end through the public API: this is the shape a manually
        // pasted `{"command": "node D:\\...\\x.js"}` JSON config produces.
        let svc = make_service();
        let created = svc
            .add_server(
                TEST_USER_ID,
                CreateMcpServerRequest {
                    name: "one-web-tools".into(),
                    description: None,
                    transport: McpTransport::Stdio {
                        command: r"node D:\1one-command\out\main\builtin-mcp-web-tools.js".into(),
                        args: vec![],
                        env: HashMap::new(),
                    },
                    original_json: None,
                    builtin: false,
                },
            )
            .await
            .unwrap();

        match created.transport {
            McpTransport::Stdio { command, args, .. } => {
                assert_eq!(command, "node");
                assert_eq!(args, vec![r"D:\1one-command\out\main\builtin-mcp-web-tools.js"]);
            }
            _ => panic!("expected stdio transport"),
        }
    }

    /// The other half of the same split: on Unix `\` is an escape, so a command
    /// written with escaped spaces has to survive as ONE argument. Asserting
    /// only the Windows behaviour left this side of the `cfg` untested.
    #[cfg(unix)]
    #[tokio::test]
    async fn add_server_keeps_posix_escapes_as_one_argument() {
        let svc = make_service();
        let created = svc
            .add_server(
                TEST_USER_ID,
                CreateMcpServerRequest {
                    name: "one-web-tools".into(),
                    description: None,
                    transport: McpTransport::Stdio {
                        command: r"node /opt/one\ command/main.js".into(),
                        args: vec![],
                        env: HashMap::new(),
                    },
                    original_json: None,
                    builtin: false,
                },
            )
            .await
            .unwrap();

        match created.transport {
            McpTransport::Stdio { command, args, .. } => {
                assert_eq!(command, "node");
                assert_eq!(args, vec!["/opt/one command/main.js"]);
            }
            _ => panic!("expected stdio transport"),
        }
    }

    #[tokio::test]
    async fn batch_import_requires_explicit_activation() {
        let svc = make_service();
        let mut req = stdio_import_req("enabled-mcp");
        req.enabled = Some(true);
        let result = svc
            .batch_import(TEST_USER_ID, BatchImportMcpServersRequest { servers: vec![req] })
            .await
            .unwrap();

        assert_eq!(result[0].name, "enabled-mcp");
        assert!(!result[0].enabled);
    }

    #[tokio::test]
    async fn batch_import_empty_list() {
        let svc = make_service();
        let req = BatchImportMcpServersRequest { servers: vec![] };
        let results = svc.batch_import(TEST_USER_ID, req).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn persist_test_result_records_success_status_and_tools() {
        let svc = make_service();
        let created = svc
            .add_server(TEST_USER_ID, stdio_create_req("persist-success"))
            .await
            .unwrap();
        let result = McpConnectionTestResult {
            success: true,
            tools: Some(vec![dream_core_api_types::McpToolResponse::new(
                "read_file".into(),
                Some("Read a file".into()),
                None,
            )]),
            error: None,
            code: None,
            details: None,
            needs_auth: None,
            auth_method: None,
            www_authenticate: None,
        };

        svc.persist_test_result(TEST_USER_ID, &created.id, &result)
            .await
            .unwrap();

        let updated = svc.get_server(TEST_USER_ID, &created.id).await.unwrap();
        assert_eq!(updated.last_test_status, dream_core_common::McpServerStatus::Connected);
        assert_eq!(updated.tools.unwrap().len(), 1);
        assert!(updated.last_connected.is_some());
    }

    #[tokio::test]
    async fn persist_test_result_records_error_and_clears_tools() {
        let svc = make_service();
        let created = svc
            .add_server(TEST_USER_ID, stdio_create_req("persist-error"))
            .await
            .unwrap();

        let success = McpConnectionTestResult {
            success: true,
            tools: Some(vec![dream_core_api_types::McpToolResponse::new(
                "read_file".into(),
                Some("Read a file".into()),
                None,
            )]),
            error: None,
            code: None,
            details: None,
            needs_auth: None,
            auth_method: None,
            www_authenticate: None,
        };
        svc.persist_test_result(TEST_USER_ID, &created.id, &success)
            .await
            .unwrap();

        let failure = McpConnectionTestResult {
            success: false,
            tools: None,
            error: Some("boom".into()),
            code: None,
            details: None,
            needs_auth: None,
            auth_method: None,
            www_authenticate: None,
        };
        svc.persist_test_result(TEST_USER_ID, &created.id, &failure)
            .await
            .unwrap();

        let updated = svc.get_server(TEST_USER_ID, &created.id).await.unwrap();
        assert_eq!(updated.last_test_status, dream_core_common::McpServerStatus::Error);
        assert!(updated.tools.is_none());
        assert!(updated.last_connected.is_some());
    }
}

#[cfg(test)]
mod team_sync_tests {
    use super::tests::MockMcpServerRepo;
    use super::*;

    const TEST_USER_ID: &str = "user-1";

    fn svc() -> McpConfigService {
        McpConfigService::new(Arc::new(MockMcpServerRepo::new()))
    }

    /// Answering an identical request from memory must not hide a change of
    /// authority: the same payloads that were kept offline are pruned once the
    /// server is reachable again.
    #[tokio::test]
    async fn identical_team_sync_is_memoised_but_authority_change_still_applies() {
        let repo = Arc::new(MockMcpServerRepo::new());
        let svc = McpConfigService::new(repo.clone());
        let both = [
            payload("omcp_a", "team-search", "sse", "https://mcp.corp/sse"),
            payload("omcp_b", "team-tools", "stdio", "npx corp-tools --serve"),
        ];
        svc.sync_team_servers(TEST_USER_ID, &both, true).await.unwrap();
        let again = svc.sync_team_servers(TEST_USER_ID, &both, true).await.unwrap();
        assert_eq!(again.written.len(), 2);

        // Server now serves only one; offline pass keeps the other.
        let one = &both[..1];
        let offline = svc.sync_team_servers(TEST_USER_ID, one, false).await.unwrap();
        assert!(offline.removed.is_empty());
        // Same payloads, authoritative: the dropped connector goes.
        let online = svc.sync_team_servers(TEST_USER_ID, one, true).await.unwrap();
        assert_eq!(online.removed, vec!["team-tools".to_owned()]);
    }

    fn payload(id: &str, name: &str, ty: &str, endpoint: &str) -> TeamMcpPayload {
        TeamMcpPayload {
            registry_id: id.to_owned(),
            name: name.to_owned(),
            server_type: ty.to_owned(),
            endpoint: endpoint.to_owned(),
            enabled: true,
            secrets_json: None,
            employee_bound: false,
        }
    }

    #[tokio::test]
    async fn materializes_team_connectors_locally() {
        let svc = svc();
        let report = svc
            .sync_team_servers(
                TEST_USER_ID,
                &[
                    payload("omcp_a", "team-search", "sse", "https://mcp.corp/sse"),
                    payload("omcp_b", "team-tools", "stdio", "npx corp-tools --serve"),
                ],
                true,
            )
            .await
            .unwrap();
        assert_eq!(report.written, vec!["team-search".to_owned(), "team-tools".to_owned()]);
        assert!(report.conflicts.is_empty());

        let listed = svc.list_servers(TEST_USER_ID).await.unwrap();
        let search = listed.iter().find(|s| s.name == "team-search").unwrap();
        assert!(search.enabled, "team connector materialized enabled");
        assert!(
            search.original_json.as_deref().unwrap_or("").contains("omcp_a"),
            "ownership marker stored"
        );
    }

    #[tokio::test]
    async fn employee_bound_connectors_sit_outside_the_member_quota() {
        let svc = svc();
        let mut payloads: Vec<TeamMcpPayload> = (0..20)
            .map(|i| {
                payload(
                    &format!("omcp_s{i}"),
                    &format!("sel-{i}"),
                    "sse",
                    &format!("https://s{i}/sse"),
                )
            })
            .collect();
        for i in 0..3 {
            let mut bound = payload(
                &format!("omcp_b{i}"),
                &format!("bound-{i}"),
                "sse",
                &format!("https://b{i}/sse"),
            );
            bound.employee_bound = true;
            payloads.push(bound);
        }
        svc.sync_team_servers(TEST_USER_ID, &payloads, true).await.unwrap();

        let rows = svc.repo.list(TEST_USER_ID).await.unwrap();
        assert_eq!(rows.iter().filter(|r| r.enabled && !r.builtin).count(), 20);
        assert_eq!(rows.iter().filter(|r| !r.enabled && !r.builtin).count(), 3);
        // The employee run still resolves a bound connector although it is disabled.
        assert!(
            svc.team_server_transport(TEST_USER_ID, "omcp_b1")
                .await
                .unwrap()
                .is_some()
        );

        // A plain disabled team row (not employee-bound) must not resolve.
        let mut disabled = payload("omcp_plain", "plain", "sse", "https://plain/sse");
        disabled.enabled = false;
        svc.sync_team_servers(TEST_USER_ID, &[disabled], true).await.unwrap();
        assert!(
            svc.team_server_transport(TEST_USER_ID, "omcp_plain")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn employee_bound_bucket_has_its_own_twenty_entry_ceiling() {
        let svc = svc();
        let bound: Vec<TeamMcpPayload> = (0..21)
            .map(|i| {
                let mut p = payload(
                    &format!("omcp_b{i}"),
                    &format!("bound-{i}"),
                    "sse",
                    &format!("https://b{i}/sse"),
                );
                p.employee_bound = true;
                p
            })
            .collect();
        let err = svc.sync_team_servers(TEST_USER_ID, &bound, true).await.unwrap_err();
        assert!(err.to_string().contains("MCP_LIMIT_EXCEEDED"), "got {err}");
        assert!(
            svc.repo
                .list(TEST_USER_ID)
                .await
                .unwrap()
                .iter()
                .all(|r| team_registry_id_of(r.original_json.as_deref()).is_none()),
            "a rejected sync must write nothing"
        );
    }

    #[tokio::test]
    async fn team_server_transport_resolves_by_registry_id_only() {
        let svc = svc();
        svc.sync_team_servers(
            TEST_USER_ID,
            &[payload("omcp_a", "team-search", "sse", "https://mcp.corp/sse")],
            true,
        )
        .await
        .unwrap();

        let resolved = svc.team_server_transport(TEST_USER_ID, "omcp_a").await.unwrap();
        let local_id = svc.list_servers(TEST_USER_ID).await.unwrap()[0].id.clone();
        assert!(
            matches!(&resolved, Some((id, McpServerTransport::Sse { url, .. })) if *id == local_id && url == "https://mcp.corp/sse"),
            "materialized connector resolves by its registry id, got {resolved:?}"
        );
        assert!(
            svc.team_server_transport(TEST_USER_ID, "omcp_missing")
                .await
                .unwrap()
                .is_none()
        );
        // The local row id is not a registry id; resolving by it must not match.
        assert!(
            svc.team_server_transport(TEST_USER_ID, &local_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn resync_with_no_changes_writes_nothing() {
        let repo = Arc::new(MockMcpServerRepo::new());
        let svc = McpConfigService::new(repo.clone());
        let payloads = [
            payload("omcp_a", "team-search", "sse", "https://mcp.corp/sse"),
            payload("omcp_b", "team-tools", "stdio", "npx corp-tools --serve"),
        ];

        svc.sync_team_servers(TEST_USER_ID, &payloads, true).await.unwrap();
        let after_first_pass = repo.write_call_count();
        assert_eq!(after_first_pass, 2, "first pass creates both connectors");

        // Same registry state next cycle (nothing an admin touched) — this is
        // the steady-state case that used to cost a DB upsert per connector
        // every 5 minutes regardless of whether anything changed.
        let report = svc.sync_team_servers(TEST_USER_ID, &payloads, true).await.unwrap();
        assert_eq!(
            repo.write_call_count(),
            after_first_pass,
            "unchanged resync must not touch the repo"
        );
        assert_eq!(report.written, vec!["team-search".to_owned(), "team-tools".to_owned()]);
        assert!(report.conflicts.is_empty());

        // A real change (admin re-pointed the endpoint) still writes through.
        let mut changed = payloads.to_vec();
        changed[0] = payload("omcp_a", "team-search", "sse", "https://mcp.corp/sse-v2");
        svc.sync_team_servers(TEST_USER_ID, &changed, true).await.unwrap();
        assert_eq!(
            repo.write_call_count(),
            after_first_pass + 1,
            "the one connector that actually changed writes; the other still doesn't"
        );
    }

    #[tokio::test]
    async fn distributed_secrets_materialize_into_transport() {
        let svc = svc();
        let mut p = payload("omcp_s", "team-auth", "sse", "https://mcp.corp/sse");
        p.secrets_json = Some(r#"{"Authorization":"Bearer team-token"}"#.to_owned());
        svc.sync_team_servers(TEST_USER_ID, &[p], true).await.unwrap();

        let server = svc
            .list_servers(TEST_USER_ID)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.name == "team-auth")
            .unwrap();
        match server.transport {
            dream_core_api_types::McpTransport::Sse { headers, .. } => {
                assert_eq!(
                    headers.get("Authorization").map(String::as_str),
                    Some("Bearer team-token")
                );
            }
            other => panic!("expected sse transport, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn never_clobbers_personal_server_with_same_name() {
        let svc = svc();
        // Member's own personal server.
        svc.add_server(
            TEST_USER_ID,
            CreateMcpServerRequest {
                name: "my-mcp".to_owned(),
                description: None,
                transport: dream_core_api_types::McpTransport::Sse {
                    url: "https://personal.example/sse".to_owned(),
                    headers: Default::default(),
                },
                original_json: None,
                builtin: false,
            },
        )
        .await
        .unwrap();

        let report = svc
            .sync_team_servers(
                TEST_USER_ID,
                &[payload("omcp_x", "my-mcp", "sse", "https://corp.example/sse")],
                true,
            )
            .await
            .unwrap();
        assert_eq!(report.conflicts, vec!["my-mcp".to_owned()]);

        let listed = svc.list_servers(TEST_USER_ID).await.unwrap();
        let mine = listed.iter().find(|s| s.name == "my-mcp").unwrap();
        assert!(mine.original_json.is_none(), "personal server untouched by team sync");
    }

    #[tokio::test]
    async fn authoritative_resync_removes_admin_deleted_and_offline_keeps() {
        let svc = svc();
        svc.sync_team_servers(
            TEST_USER_ID,
            &[
                payload("omcp_a", "team-a", "sse", "https://a/sse"),
                payload("omcp_b", "team-b", "sse", "https://b/sse"),
            ],
            true,
        )
        .await
        .unwrap();

        // Offline pass (not authoritative): nothing removed.
        let offline = svc.sync_team_servers(TEST_USER_ID, &[], false).await.unwrap();
        assert!(offline.removed.is_empty());
        assert_eq!(svc.list_servers(TEST_USER_ID).await.unwrap().len(), 2);

        // Admin deleted team-b on the server: authoritative resync removes it.
        let resync = svc
            .sync_team_servers(
                TEST_USER_ID,
                &[payload("omcp_a", "team-a", "sse", "https://a/sse")],
                true,
            )
            .await
            .unwrap();
        assert_eq!(resync.removed, vec!["team-b".to_owned()]);
        let names: Vec<String> = svc
            .list_servers(TEST_USER_ID)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["team-a".to_owned()]);
    }
}
