use std::sync::Arc;
use std::sync::Mutex;

use serde_json::json;

use super::*;

#[derive(Default)]
struct RecordingGate {
    verdict: Option<String>,
    error: Option<String>,
    seen: Mutex<Vec<(String, bool)>>,
}

#[async_trait]
impl ToolCallSecurityGate for RecordingGate {
    async fn check(
        &self,
        _user_id: &str,
        command_text: &str,
        _is_network_fetch: bool,
        is_terminal_tool: bool,
    ) -> Result<Option<String>, String> {
        self.seen
            .lock()
            .unwrap()
            .push((command_text.to_owned(), is_terminal_tool));
        match &self.error {
            Some(error) => Err(error.clone()),
            None => Ok(self.verdict.clone()),
        }
    }
}

/// The case real-machine testing caught: a blocked command in full-auto.
#[tokio::test]
async fn a_blocked_command_is_refused_with_the_policy_reason() {
    let gate = Arc::new(RecordingGate {
        verdict: Some("blocked by company policy: AUDITBLOCKED".into()),
        ..Default::default()
    });
    let guard = SecurityPolicyCallGuard::new(gate, "u1");

    let refusal = guard.check("ExecCommand", &json!({"cmd": "echo AUDITBLOCKED"})).await;

    assert_eq!(refusal.as_deref(), Some("blocked by company policy: AUDITBLOCKED"));
}

#[tokio::test]
async fn an_allowed_command_runs() {
    let gate = Arc::new(RecordingGate::default());
    let guard = SecurityPolicyCallGuard::new(gate, "u1");

    assert!(guard.check("ExecCommand", &json!({"cmd": "ls"})).await.is_none());
}

/// The command text has to carry the argument, or every pattern in the policy
/// matches nothing — the whole failure this guard exists to fix.
#[tokio::test]
async fn the_tool_input_reaches_the_policy() {
    let gate = Arc::new(RecordingGate::default());
    let guard = SecurityPolicyCallGuard::new(gate.clone(), "u1");

    guard.check("ExecCommand", &json!({"cmd": "rm -rf /tmp/x"})).await;

    let seen = gate.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].0.contains("rm -rf /tmp/x"), "command text was {:?}", seen[0].0);
    assert!(seen[0].1, "ExecCommand must be reported as a terminal tool");
}

#[tokio::test]
async fn only_the_shell_tool_counts_as_a_terminal_tool() {
    let gate = Arc::new(RecordingGate::default());
    let guard = SecurityPolicyCallGuard::new(gate.clone(), "u1");

    guard.check("Read", &json!({"path": "a.txt"})).await;

    assert!(!gate.seen.lock().unwrap()[0].1);
}

/// A lookup that failed is not a lookup that allowed.
#[tokio::test]
async fn a_failed_check_refuses_the_call() {
    let gate = Arc::new(RecordingGate {
        error: Some("database is locked".into()),
        ..Default::default()
    });
    let guard = SecurityPolicyCallGuard::new(gate, "u1");

    let refusal = guard.check("ExecCommand", &json!({"cmd": "ls"})).await;

    assert!(refusal.unwrap().contains("database is locked"));
}

#[tokio::test]
async fn cached_mcp_access_tracks_disable_restore_and_delete_in_the_live_repository() {
    use dream_core_db::{CreateMcpServerParams, IMcpServerRepository, UpdateMcpServerParams};
    let db = dream_core_db::init_database_memory().await.unwrap();
    let repo = dream_core_db::SqliteMcpServerRepository::new(db.pool().clone(), [3; 32]);
    let user = "system_default_user";
    let row = repo
        .create(CreateMcpServerParams {
            user_id: user,
            name: "company-test",
            description: None,
            enabled: true,
            transport_type: "stdio",
            transport_config: r#"{"command":"node","args":[]}"#,
            tools: None,
            original_json: Some(r#"{"teamRegistryId":"omcp-test"}"#),
            builtin: false,
        })
        .await
        .unwrap();
    assert!(team_mcp_refusal(&repo, user, &row.id).await.is_none());
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
    assert!(team_mcp_refusal(&repo, user, &row.id).await.is_some());
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
    assert!(team_mcp_refusal(&repo, user, &row.id).await.is_none());
    repo.delete(user, &row.id).await.unwrap();
    assert!(team_mcp_refusal(&repo, user, &row.id).await.is_some());
}

#[tokio::test]
async fn registry_names_enforce_revocation_for_normalized_collisions_and_preserve_other_tools() {
    use dream_core_db::{CreateMcpServerParams, IMcpServerRepository, UpdateMcpServerParams};
    use dream_engine_mcp::{manager::McpManager, tool_proxy::McpToolProxy};
    use dream_engine_tools::{read::ReadTool, registry::ToolRegistry};
    let db = dream_core_db::init_database_memory().await.unwrap();
    let repo = Arc::new(dream_core_db::SqliteMcpServerRepository::new(
        db.pool().clone(),
        [4; 32],
    ));
    let user = "system_default_user";
    let row = repo
        .create(CreateMcpServerParams {
            user_id: user,
            name: "company",
            description: None,
            enabled: false,
            transport_type: "stdio",
            transport_config: r#"{"command":"node","args":[]}"#,
            tools: None,
            original_json: Some(r#"{"teamRegistryId":"omcp-test"}"#),
            builtin: false,
        })
        .await
        .unwrap();
    let manager = Arc::new(McpManager::connect_all(&Default::default()).await.unwrap());
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(None)));
    let long_server = "a very long company connector name".repeat(4);
    let cases = [
        ("company", "ping", "ping"),
        ("company", "Read", "mcp__company_Read"),
        ("企业费用核算", "Read", "mcp___12345678_Read"),
        (long_server.as_str(), "Read", "mcp__a_87654321_Read"),
    ];
    let mut endpoints = Vec::new();
    for (server, original, registered) in cases {
        registry.register(Box::new(McpToolProxy::new(
            registered.into(),
            original.into(),
            server.into(),
            "test".into(),
            json!({"type":"object"}),
            manager.clone(),
            false,
        )));
        endpoints.push((server.into(), original.into(), row.id.clone()));
    }
    registry.register(Box::new(McpToolProxy::new(
        "personal_tool".into(),
        "personal_tool".into(),
        "personal".into(),
        "test".into(),
        json!({"type":"object"}),
        manager,
        false,
    )));
    let bindings = team_mcp_tool_bindings(&registry, &endpoints);
    assert_eq!(bindings.len(), 4);
    let gate = Arc::new(RecordingGate::default());
    let guard = TeamMcpCallGuard {
        inner: Some(Arc::new(SecurityPolicyCallGuard::new(gate.clone(), user))),
        repo: repo.clone(),
        user_id: user.into(),
        tool_server_ids: bindings.clone(),
    };
    for name in bindings.keys() {
        assert!(
            guard.check(name, &json!({})).await.unwrap().contains("revoked"),
            "{name}"
        );
    }
    assert!(guard.check("Read", &json!({})).await.is_none());
    assert!(guard.check("personal_tool", &json!({})).await.is_none());
    assert_eq!(
        gate.seen.lock().unwrap().len(),
        2,
        "unrelated tools still run the existing company guard"
    );
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
    for name in bindings.keys() {
        assert!(guard.check(name, &json!({})).await.is_none());
    }
    assert_eq!(gate.seen.lock().unwrap().len(), 6);
}
