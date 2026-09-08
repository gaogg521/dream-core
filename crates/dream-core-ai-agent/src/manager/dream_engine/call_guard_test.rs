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
