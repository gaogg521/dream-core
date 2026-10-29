//! Neutral MCP server resolution (Wave 0c E) — the SINGLE source of truth for
//! turning a conversation's configured MCP servers into the SDK-free
//! [`SessionMcpServer`] shape the clean-slate session stack carries in
//! `SessionConfig.init.mcp_servers`.
//!
//! The legacy per-backend resolvers (`factory::acp::load_user_mcp_servers` →
//! `Vec<agent_client_protocol::McpServer>`, `factory::dream_engine::load_user_mcp_servers`
//! → `HashMap<String, McpServerConfig>`) emit SDK/engine-specific types. This
//! module emits the NEUTRAL `dream_core_api_types::SessionMcpServer` so the app
//! boundary (`dream-app`) can convert it once into the crate-local
//! `dream_core_session::McpServerSpec`, and each backend serializes that into its own
//! wire shape. Same row-walking + selection + stdio-launch-resolution logic as
//! the legacy ACP path, but vendor-neutral.

use std::sync::Arc;

use dream_core_api_types::{SessionMcpServer, SessionMcpTransport, is_team_mcp_server_name};
use dream_core_db::IMcpServerRepository;
use dream_core_db::models::McpServerRow;
use dream_core_realtime::EventBroadcaster;
use dream_core_runtime::ensure_runtime_command;
use tracing::{info, warn};

/// Resolve a conversation's user-configured MCP servers into neutral
/// [`SessionMcpServer`]s. `selected_ids = Some` → that frozen snapshot defines the
/// session (injected regardless of the row's global `enabled` flag); `None` → all
/// enabled rows. Builtin rows are excluded (guide/team MCP are folded separately
/// by the caller). Stdio launch commands are RESOLVED here (e.g. `npx` → the
/// bundled-node absolute path) so the spec that reaches `open_session` is final —
/// the Wave 0c contract that `McpTransport::Stdio.command` is pre-resolved.
///
/// Best-effort: a repo error, a capability-unsupported transport, or a malformed
/// `transport_config` row is warn-logged and skipped, never fatal. `broadcaster`
/// is accepted for parity with the legacy reporter path (runtime-resolution status
/// reporting) and reserved for that use.
pub async fn resolve_session_mcp_servers(
    repo: &dyn IMcpServerRepository,
    user_id: &str,
    selected_ids: Option<&[String]>,
    conversation_id: &str,
    _broadcaster: Arc<dyn EventBroadcaster>,
) -> Vec<SessionMcpServer> {
    let rows_result = match selected_ids {
        Some(ids) => repo.list_by_ids_any(user_id, ids).await,
        None => repo.list(user_id).await,
    };
    let mut rows = match rows_result {
        Ok(r) => r,
        Err(err) => {
            warn!(conversation_id, error = %err, "mcp_resolve: list() failed; skipping injection");
            return Vec::new();
        }
    };

    if rows
        .iter()
        .filter(|row| row.enabled && dream_core_common::mcp_policy::is_enterprise_mcp(row.original_json.as_deref()))
        .count()
        > dream_core_common::mcp_policy::MAX_USER_MCP_SERVERS
    {
        warn!(
            conversation_id,
            "Over-limit enterprise MCP selection blocked before command resolution"
        );
        rows.retain(|row| !dream_core_common::mcp_policy::is_enterprise_mcp(row.original_json.as_deref()));
    }

    let mut servers = Vec::with_capacity(rows.len());
    for mut row in rows {
        let selected = selected_ids
            .map(|ids| ids.iter().any(|id| id == &row.id))
            .unwrap_or(false);
        // `dream-team` is a reserved wire-level name: the team coordination MCP
        // must win, so a user row that collides with it is skipped (never
        // injected), regardless of selection state.
        if !selected || !row.enabled || row.builtin || row.deleted_at.is_some() || is_team_mcp_server_name(&row.name) {
            continue;
        }
        if authorize_runtime_row(repo, user_id, &mut row).await.is_err() {
            continue;
        }
        match row_to_session_mcp_server(&row).await {
            Ok(server) => servers.push(server),
            Err(err) => {
                warn!(
                    conversation_id,
                    server_id = %row.id,
                    server_name = %row.name,
                    error = %err,
                    "mcp_resolve: failed to convert row; skipping"
                );
            }
        }
    }

    if !servers.is_empty() {
        info!(
            conversation_id,
            count = servers.len(),
            "mcp_resolve: resolved user MCP servers"
        );
    }
    servers
}

/// Refresh app-owned tool transports and inject current native-HTTP credentials into the runtime copy.
pub(crate) async fn authorize_session_server(
    repo: &dyn IMcpServerRepository,
    user: &str,
    server: &mut SessionMcpServer,
) -> Result<(), String> {
    // These local app services carry per-instance ports in their transport.
    // Persisted snapshots must not reconnect to another instance after restart.
    if matches!(
        server.name.as_str(),
        "one-browser" | "one-page-reader" | "one-export-pdf" | "one-html-render"
    ) {
        let row = repo.find_by_name(user, &server.name).await.map_err(|e| e.to_string())?;
        let Some(row) = row.filter(|row| row.builtin && row.enabled && row.deleted_at.is_none()) else {
            warn!(server_name = %server.name, "Builtin MCP is unavailable; refusing stored transport");
            return Err("Builtin MCP is unavailable".into());
        };
        let current = row_to_session_mcp_server(&row).await?;
        if serde_json::to_value(&server.transport).ok() != serde_json::to_value(&current.transport).ok() {
            info!(server_name = %server.name, "Refreshed builtin MCP transport from current app registration");
        }
        *server = current;
        return Ok(());
    }
    let (url, headers) = match &mut server.transport {
        SessionMcpTransport::StreamableHttp { url, headers } | SessionMcpTransport::Sse { url, headers } => {
            (url, headers)
        }
        _ => return Ok(()),
    };
    if !headers.keys().any(|key| key.eq_ignore_ascii_case("authorization"))
        && let Some(token) = repo.oauth_bearer_token(user, url).await.map_err(|e| e.to_string())?
    {
        headers.insert("Authorization".into(), format!("Bearer {token}"));
    }
    Ok(())
}

pub(crate) async fn authorize_runtime_row(
    repo: &dyn IMcpServerRepository,
    user: &str,
    row: &mut McpServerRow,
) -> Result<(), String> {
    if !matches!(row.transport_type.as_str(), "http" | "streamable_http" | "sse") {
        return Ok(());
    }
    let mut value: serde_json::Value = serde_json::from_str(&row.transport_config).map_err(|e| e.to_string())?;
    let Some(url) = value.get("url").and_then(|v| v.as_str()) else {
        return Ok(());
    };
    let token = repo.oauth_bearer_token(user, url).await.map_err(|e| e.to_string())?;
    if let Some(token) = token {
        if value.get("headers").is_none_or(|v| v.is_null()) {
            value["headers"] = serde_json::json!({});
        }
        if let Some(headers) = value.get_mut("headers").and_then(|v| v.as_object_mut())
            && !headers.keys().any(|key| key.eq_ignore_ascii_case("authorization"))
        {
            headers.insert("Authorization".into(), serde_json::json!(format!("Bearer {token}")));
        }
        row.transport_config = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Parse one `McpServerRow` into a neutral `SessionMcpServer`, resolving the stdio
/// launch command. Mirrors `factory::acp::row_to_sdk_mcp_server` but emits the
/// neutral type. Returns an error string when `transport_config` is malformed.
///
/// This is the SINGLE shared row→neutral conversion: `dream-conversation` (team
/// snapshot refresh) and the session stack reuse it so the stdio command is
/// always normalized exactly once, the same way the direct claude/codex path
/// consumes it.
pub async fn row_to_session_mcp_server(row: &McpServerRow) -> Result<SessionMcpServer, String> {
    let value: serde_json::Value =
        serde_json::from_str(&row.transport_config).map_err(|e| format!("invalid transport_config JSON: {e}"))?;

    let transport = match row.transport_type.as_str() {
        "stdio" => {
            let command = value
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "stdio: missing command".to_owned())?;
            let args: Vec<String> = value
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            if dream_core_common::mcp_policy::is_browser_auth_proxy(command, &args) {
                return Err("MCP_AUTH_REQUIRED: configure native HTTP transport and authorize explicitly; browser proxy auto-start is disabled".into());
            }
            let mut env: std::collections::HashMap<String, String> = value
                .get("env")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                        .collect()
                })
                .unwrap_or_default();

            // Resolve the launch command (npx/bun → bundled path) + fold in the
            // runtime-provided args prefix + env, exactly like the legacy
            // `ensure_stdio_launch`. The resolved form is what the agent spawns.
            let resolved = ensure_runtime_command(command).await.map_err(|e| e.to_string())?;
            let mut final_args: Vec<String> = resolved
                .args_prefix
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            final_args.extend(args);
            for (k, v) in resolved.env {
                env.insert(k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned());
            }
            SessionMcpTransport::Stdio {
                command: resolved.program.to_string_lossy().into_owned(),
                args: final_args,
                env,
            }
        }
        "http" | "streamable_http" => {
            let url = value
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "http: missing url".to_owned())?
                .to_owned();
            SessionMcpTransport::StreamableHttp {
                url,
                headers: parse_headers(value.get("headers")),
            }
        }
        "sse" => {
            let url = value
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "sse: missing url".to_owned())?
                .to_owned();
            SessionMcpTransport::Sse {
                url,
                headers: parse_headers(value.get("headers")),
            }
        }
        other => return Err(format!("unknown transport type: {other}")),
    };

    Ok(SessionMcpServer {
        id: row.id.clone(),
        name: row.name.clone(),
        transport,
    })
}

/// Parse a JSON headers object into a `HashMap` (string values only).
fn parse_headers(value: Option<&serde_json::Value>) -> std::collections::HashMap<String, String> {
    value
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_api_types::TEAM_MCP_SERVER_NAME;
    use dream_core_db::models::McpServerRow;

    const TEST_USER_ID: &str = "user-1";

    struct MockRepo {
        rows: Vec<McpServerRow>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl IMcpServerRepository for MockRepo {
        async fn list(&self, user_id: &str) -> Result<Vec<McpServerRow>, dream_core_db::DbError> {
            if self.fail {
                return Err(dream_core_db::DbError::Init("boom".into()));
            }
            Ok(self.rows.iter().filter(|row| row.user_id == user_id).cloned().collect())
        }
        async fn find_by_id(&self, user_id: &str, id: &str) -> Result<Option<McpServerRow>, dream_core_db::DbError> {
            Ok(self
                .rows
                .iter()
                .find(|row| row.user_id == user_id && row.id == id)
                .cloned())
        }
        async fn find_by_name(
            &self,
            user_id: &str,
            name: &str,
        ) -> Result<Option<McpServerRow>, dream_core_db::DbError> {
            Ok(self
                .rows
                .iter()
                .find(|row| row.user_id == user_id && row.name == name)
                .cloned())
        }
        async fn create(
            &self,
            _params: dream_core_db::CreateMcpServerParams<'_>,
        ) -> Result<McpServerRow, dream_core_db::DbError> {
            unimplemented!("not needed")
        }
        async fn update(
            &self,
            _user_id: &str,
            _id: &str,
            _params: dream_core_db::UpdateMcpServerParams<'_>,
        ) -> Result<McpServerRow, dream_core_db::DbError> {
            unimplemented!("not needed")
        }
        async fn delete(&self, _user_id: &str, _id: &str) -> Result<(), dream_core_db::DbError> {
            unimplemented!("not needed")
        }
        async fn batch_upsert(
            &self,
            _user_id: &str,
            _servers: &[dream_core_db::CreateMcpServerParams<'_>],
        ) -> Result<Vec<McpServerRow>, dream_core_db::DbError> {
            unimplemented!("not needed")
        }
        async fn update_status(
            &self,
            _user_id: &str,
            _id: &str,
            _status: &str,
            _last_connected: Option<dream_core_common::TimestampMs>,
        ) -> Result<(), dream_core_db::DbError> {
            unimplemented!("not needed")
        }
        async fn update_tools(
            &self,
            _user_id: &str,
            _id: &str,
            _tools: Option<&str>,
        ) -> Result<(), dream_core_db::DbError> {
            unimplemented!("not needed")
        }
    }

    fn make_row(name: &str, enabled: bool) -> McpServerRow {
        McpServerRow {
            id: format!("mcp_{name}"),
            user_id: TEST_USER_ID.to_owned(),
            name: name.to_owned(),
            description: None,
            enabled,
            transport_type: "http".into(),
            transport_config: r#"{"url":"http://127.0.0.1:9999/mcp"}"#.into(),
            tools: None,
            last_test_status: "disconnected".into(),
            last_connected: None,
            original_json: None,
            builtin: false,
            deleted_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn test_broadcaster() -> Arc<dyn EventBroadcaster> {
        Arc::new(dream_core_realtime::BroadcastEventBus::new(16))
    }

    #[tokio::test]
    async fn builtin_snapshot_uses_current_transport_and_cannot_revive_disabled_or_deleted_tools() {
        let mut row = make_row("one-export-pdf", true);
        row.builtin = true;
        let original = SessionMcpServer {
            id: row.id.clone(),
            name: row.name.clone(),
            transport: SessionMcpTransport::StreamableHttp {
                url: "http://127.0.0.1:19820/old".into(),
                headers: Default::default(),
            },
        };
        let mut current = original.clone();
        authorize_session_server(
            &MockRepo {
                rows: vec![row.clone()],
                fail: false,
            },
            TEST_USER_ID,
            &mut current,
        )
        .await
        .unwrap();
        assert!(
            matches!(current.transport, SessionMcpTransport::StreamableHttp { ref url, .. } if url == "http://127.0.0.1:9999/mcp")
        );
        for variant in 0..4 {
            let mut unavailable = row.clone();
            match variant {
                0 => unavailable.enabled = false,
                1 => unavailable.deleted_at = Some(1),
                2 => unavailable.builtin = false,
                _ => {}
            }
            let repo = MockRepo {
                rows: if variant == 3 { vec![] } else { vec![unavailable] },
                fail: false,
            };
            assert!(
                authorize_session_server(&repo, TEST_USER_ID, &mut original.clone())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn resolves_more_than_fifty_local_connectors_and_blocks_only_enterprise_overflow() {
        let mut rows: Vec<_> = (0..100).map(|i| make_row(&format!("local-{i}"), true)).collect();
        let local_ids: Vec<_> = rows.iter().map(|row| row.id.clone()).collect();
        let local = MockRepo {
            rows: rows.clone(),
            fail: false,
        };
        assert_eq!(
            resolve_session_mcp_servers(&local, TEST_USER_ID, Some(&local_ids), "c", test_broadcaster())
                .await
                .len(),
            100
        );
        for i in 0..51 {
            let mut row = make_row(&format!("enterprise-{i}"), true);
            row.original_json = Some(format!(r#"{{"teamRegistryId":"team-{i}"}}"#));
            rows.push(row);
        }
        let ids: Vec<_> = rows.iter().map(|row| row.id.clone()).collect();
        let mixed = MockRepo { rows, fail: false };
        let actual = resolve_session_mcp_servers(&mixed, TEST_USER_ID, Some(&ids), "c", test_broadcaster()).await;
        assert_eq!(actual.len(), 100);
        assert!(actual.iter().all(|row| row.name.starts_with("local-")));
    }

    #[tokio::test]
    async fn resolve_skips_reserved_team_mcp_name() {
        let repo = MockRepo {
            rows: vec![make_row("docs", true), make_row(TEAM_MCP_SERVER_NAME, true)],
            fail: false,
        };
        let servers = resolve_session_mcp_servers(
            &repo,
            TEST_USER_ID,
            Some(&[
                "mcp_docs".into(),
                "mcp_off".into(),
                "mcp_builtin".into(),
                TEAM_MCP_SERVER_NAME.into(),
            ]),
            "conv-1",
            test_broadcaster(),
        )
        .await;
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "docs");
    }

    #[tokio::test]
    async fn resolve_skips_disabled_and_builtin_rows() {
        let repo = MockRepo {
            rows: vec![
                make_row("docs", true),
                make_row("off", false),
                McpServerRow {
                    id: "mcp_builtin".into(),
                    user_id: TEST_USER_ID.to_owned(),
                    name: "chrome-devtools".into(),
                    description: None,
                    enabled: true,
                    transport_type: "stdio".into(),
                    transport_config: r#"{"command":"/bin/true","args":[],"env":{}}"#.into(),
                    tools: None,
                    last_test_status: "disconnected".into(),
                    last_connected: None,
                    original_json: None,
                    builtin: true,
                    deleted_at: None,
                    created_at: 0,
                    updated_at: 0,
                },
            ],
            fail: false,
        };
        let servers = resolve_session_mcp_servers(
            &repo,
            TEST_USER_ID,
            Some(&[
                "mcp_docs".into(),
                "mcp_off".into(),
                "mcp_builtin".into(),
                TEAM_MCP_SERVER_NAME.into(),
            ]),
            "conv-1",
            test_broadcaster(),
        )
        .await;
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "docs");
    }
}
