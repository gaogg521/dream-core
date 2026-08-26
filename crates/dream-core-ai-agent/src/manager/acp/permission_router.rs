use crate::agent_runtime::AgentRuntime;
use crate::error::AgentError;
use crate::protocol::acp::{PermissionDecision, PermissionRequest};
use crate::protocol::events::{AgentStreamEvent, permission_request_to_event_data};
use crate::security_policy::ToolCallSecurityGate;
use agent_client_protocol::schema::v1::PermissionOptionKind as SdkPermissionOptionKind;
use agent_client_protocol::schema::v1::ToolKind as SdkToolKind;
use dream_core_api_types::TEAM_MCP_SERVER_NAME;
use dream_core_common::Confirmation;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, info, warn};

const AUTO_APPROVE_MCP_SERVERS: &[&str] = &[TEAM_MCP_SERVER_NAME];

struct PendingPermission {
    responder: oneshot::Sender<PermissionDecision>,
    confirmation: Confirmation,
}

/// Routes ACP permission requests from the protocol layer to the user
/// (via `event_tx`) and back (via `confirm`). Owns the receiver channel
/// for incoming permission requests, the pending responder map, and the
/// `closing` flag that prevents new requests from being routed after a
/// graceful shutdown has started.
pub struct PermissionRouter {
    /// Receiver for permission requests from the protocol layer.
    permission_rx: Mutex<mpsc::Receiver<PermissionRequest>>,
    /// Pending ACP permission responders and recovery data keyed by tool call ID.
    pending_permissions: StdMutex<HashMap<String, PendingPermission>>,
    /// Whether a graceful shutdown is in progress.
    closing: AtomicBool,
    /// The user this session belongs to, for the security-policy check below.
    user_id: String,
    /// Company security policy's destructive-command block and network-
    /// fetch denial, `None` for personal builds and tests — every tool call
    /// then flows through unmodified, exactly as before this existed.
    tool_call_security_gate: Option<Arc<dyn ToolCallSecurityGate>>,
}

impl PermissionRouter {
    /// Create a new permission router.
    pub fn new(
        permission_rx: mpsc::Receiver<PermissionRequest>,
        user_id: String,
        tool_call_security_gate: Option<Arc<dyn ToolCallSecurityGate>>,
    ) -> Self {
        Self {
            permission_rx: Mutex::new(permission_rx),
            pending_permissions: StdMutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            user_id,
            tool_call_security_gate,
        }
    }

    /// Start the permission handler loop.
    ///
    /// This background task receives permission requests from the protocol
    /// layer, converts them to `Permission` events, and waits for user
    /// responses routed through the `confirm()` method.
    ///
    /// `runtime` is shared with the parent manager so permission
    /// arrivals count as activity (preventing idle timeouts) via
    /// `runtime.bump_activity()`.
    pub fn start(self: &Arc<Self>, runtime: AgentRuntime) {
        let this = Arc::clone(self);

        tokio::spawn(async move {
            let mut rx = this.permission_rx.lock().await;

            while let Some(perm_req) = rx.recv().await {
                runtime.bump_activity();

                let call_id = perm_req.request.tool_call.tool_call_id.to_string();

                // Company security policy takes priority over both
                // auto-approval and the normal user-confirmation flow: a
                // blocked command must never reach either.
                if let Some(gate) = this.tool_call_security_gate.as_ref() {
                    let command_text = extract_command_text(&perm_req.request);
                    let is_network_fetch = perm_req.request.tool_call.fields.kind == Some(SdkToolKind::Fetch);
                    match gate.check(&this.user_id, &command_text, is_network_fetch).await {
                        Ok(Some(reason)) => {
                            warn!(
                                conversation_id = %runtime.conversation_id(),
                                call_id,
                                reason,
                                "ACP permission request blocked by company security policy"
                            );
                            let decision = match select_reject_option_id(&perm_req.request) {
                                Some(option_id) => PermissionDecision::Selected { option_id },
                                None => PermissionDecision::Cancelled,
                            };
                            let _ = perm_req.response_tx.send(decision);
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            // The check itself failed (e.g. a DB error) — this is
                            // NOT the same as the policy allowing the call. Fail
                            // closed on the one call rather than silently letting
                            // a potentially-destructive command through, and
                            // rather than freezing every other tool call in the
                            // process (unlike the IP allowlist, this check never
                            // gates the whole request).
                            warn!(
                                conversation_id = %runtime.conversation_id(),
                                call_id,
                                error,
                                "ACP permission security-policy check failed; blocking this call"
                            );
                            let decision = match select_reject_option_id(&perm_req.request) {
                                Some(option_id) => PermissionDecision::Selected { option_id },
                                None => PermissionDecision::Cancelled,
                            };
                            let _ = perm_req.response_tx.send(decision);
                            continue;
                        }
                    }
                }

                // Auto-approve team MCP tools without user interaction.
                if let Some(option_id) = auto_approve_option_id(&perm_req.request) {
                    info!(
                        conversation_id = %runtime.conversation_id(),
                        call_id,
                        option_id = %option_id,
                        server_name = ?extract_mcp_server_name(&perm_req.request),
                        "ACP team MCP permission auto-approved"
                    );
                    let _ = perm_req.response_tx.send(PermissionDecision::Selected { option_id });
                    continue;
                }

                let permission_event = permission_request_to_event_data(&perm_req.request);
                let confirmation = permission_event
                    .as_confirmation()
                    .expect("ACP permission events must be recoverable as confirmations");

                let mut pending = this.pending_permissions.lock().unwrap();
                if let Some(previous) = pending.insert(
                    call_id.clone(),
                    PendingPermission {
                        responder: perm_req.response_tx,
                        confirmation,
                    },
                ) {
                    let _ = previous.responder.send(PermissionDecision::Cancelled);
                }
                drop(pending);
                debug!(
                    conversation_id = %runtime.conversation_id(),
                    call_id,
                    "ACP permission pending confirmation registered"
                );

                if runtime
                    .event_sender()
                    .send(AgentStreamEvent::AcpPermission(permission_event))
                    .is_err()
                    && let Some(pending) = this.pending_permissions.lock().unwrap().remove(&call_id)
                {
                    let _ = pending.responder.send(PermissionDecision::Cancelled);
                }
            }
        });
    }

    /// Pending permission items recoverable by conversation confirmation APIs.
    pub fn get_confirmations(&self) -> Vec<Confirmation> {
        self.pending_permissions
            .lock()
            .unwrap()
            .values()
            .map(|pending| pending.confirmation.clone())
            .collect()
    }

    /// Resolve a pending permission request with the user's selected option.
    pub fn confirm(&self, call_id: &str, option_id: String, conversation_id: &str) -> Result<(), AgentError> {
        let pending = self
            .pending_permissions
            .lock()
            .unwrap()
            .remove(call_id)
            .ok_or_else(|| AgentError::bad_request(format!("Pending ACP permission not found: {call_id}")))?;

        pending
            .responder
            .send(PermissionDecision::Selected { option_id })
            .map_err(|_| AgentError::bad_request(format!("Pending ACP permission expired: {call_id}")))?;

        debug!(conversation_id = %conversation_id, call_id, "ACP permission response forwarded");
        Ok(())
    }

    /// Cancel all pending permission requests. Called during `stop()` and `kill()`.
    pub fn cancel_all(&self) {
        for (_, pending) in self.pending_permissions.lock().unwrap().drain() {
            let _ = pending.responder.send(PermissionDecision::Cancelled);
        }
    }

    /// Whether a graceful shutdown is in progress.
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    /// Mark the router as closing (graceful shutdown in progress).
    pub fn set_closing(&self) {
        self.closing.store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn insert_pending_for_test(
        &self,
        call_id: String,
        responder: oneshot::Sender<PermissionDecision>,
        confirmation: Confirmation,
    ) {
        self.pending_permissions.lock().unwrap().insert(
            call_id,
            PendingPermission {
                responder,
                confirmation,
            },
        );
    }
}

#[cfg(test)]
fn is_auto_approve_tool(request: &agent_client_protocol::schema::v1::RequestPermissionRequest) -> bool {
    auto_approve_option_id(request).is_some()
}

fn auto_approve_option_id(request: &agent_client_protocol::schema::v1::RequestPermissionRequest) -> Option<String> {
    let server_name = extract_mcp_server_name(request)?;
    if !AUTO_APPROVE_MCP_SERVERS.contains(&server_name.as_str()) {
        return None;
    }
    select_allow_option_id(request)
}

fn select_allow_option_id(request: &agent_client_protocol::schema::v1::RequestPermissionRequest) -> Option<String> {
    request
        .options
        .iter()
        .find(|option| matches!(option.kind, SdkPermissionOptionKind::AllowAlways))
        .or_else(|| {
            request
                .options
                .iter()
                .find(|option| matches!(option.kind, SdkPermissionOptionKind::AllowOnce))
        })
        .map(|option| option.option_id.to_string())
}

/// The reject option to select when the security policy blocks a call
/// outright. Prefers an explicit reject/cancel option so the agent (and
/// transcript) sees a real refusal rather than a silently dropped request;
/// falls back to [`PermissionDecision::Cancelled`] at the call site when no
/// such option is offered.
fn select_reject_option_id(request: &agent_client_protocol::schema::v1::RequestPermissionRequest) -> Option<String> {
    request
        .options
        .iter()
        .find(|option| {
            matches!(
                option.kind,
                SdkPermissionOptionKind::RejectOnce | SdkPermissionOptionKind::RejectAlways
            )
        })
        .map(|option| option.option_id.to_string())
}

/// Best-effort text to match blocked command patterns against: the tool
/// call's title plus its raw input serialized to JSON. See
/// [`ToolCallSecurityGate`]'s doc comment for why this doesn't assume a
/// specific per-agent field name.
fn extract_command_text(request: &agent_client_protocol::schema::v1::RequestPermissionRequest) -> String {
    let title = request.tool_call.fields.title.as_deref().unwrap_or_default();
    let raw_input = request
        .tool_call
        .fields
        .raw_input
        .as_ref()
        .map(|value| value.to_string())
        .unwrap_or_default();
    format!("{title} {raw_input}")
}

fn extract_mcp_server_name(request: &agent_client_protocol::schema::v1::RequestPermissionRequest) -> Option<String> {
    extract_mcp_server_from_raw_input(request).or_else(|| {
        request
            .tool_call
            .fields
            .title
            .as_deref()
            .and_then(extract_mcp_server_from_prefixed_title)
            .map(str::to_owned)
    })
}

fn extract_mcp_server_from_raw_input(
    request: &agent_client_protocol::schema::v1::RequestPermissionRequest,
) -> Option<String> {
    request
        .tool_call
        .fields
        .raw_input
        .as_ref()
        .and_then(|raw_input| raw_input.get("server_name"))
        .and_then(serde_json::Value::as_str)
        .filter(|server_name| !server_name.is_empty())
        .map(str::to_owned)
}

fn extract_mcp_server_from_prefixed_title(title: &str) -> Option<&str> {
    let rest = title.strip_prefix("mcp__")?;
    let (server_name, tool_name) = rest.split_once("__")?;
    if server_name.is_empty() || tool_name.is_empty() {
        return None;
    }
    Some(server_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::events::AgentStreamEvent;
    use agent_client_protocol::schema::v1::{
        PermissionOption, PermissionOptionKind as SdkPermissionOptionKind, RequestPermissionRequest,
        ToolCallUpdate as SdkToolCallUpdate, ToolCallUpdateFields, ToolKind as SdkToolKind,
    };
    use dream_core_common::Confirmation;
    use serde_json::json;
    use std::time::Duration;

    fn permission_request_with_title_and_raw_input(
        title: &str,
        raw_input: Option<serde_json::Value>,
        options: Vec<PermissionOption>,
    ) -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            "session-1",
            SdkToolCallUpdate::new(
                "tool-1",
                ToolCallUpdateFields::new()
                    .kind(SdkToolKind::Other)
                    .title(title.to_owned())
                    .raw_input(raw_input),
            ),
            options,
        )
    }

    fn allow_always_option(option_id: &'static str) -> PermissionOption {
        PermissionOption::new(
            option_id,
            "Allow for this session",
            SdkPermissionOptionKind::AllowAlways,
        )
    }

    fn allow_once_option(option_id: &'static str) -> PermissionOption {
        PermissionOption::new(option_id, "Allow", SdkPermissionOptionKind::AllowOnce)
    }

    fn reject_option(option_id: &'static str) -> PermissionOption {
        PermissionOption::new(option_id, "Reject", SdkPermissionOptionKind::RejectOnce)
    }

    fn sample_confirmation(call_id: &str) -> Confirmation {
        Confirmation {
            id: call_id.to_owned(),
            call_id: call_id.to_owned(),
            questions: None,
            title: Some("Write file".to_owned()),
            action: None,
            description: "Write /tmp/current_time.txt".to_owned(),
            command_type: Some("edit".to_owned()),
            options: vec![dream_core_common::ConfirmationOption {
                label: "Allow".to_owned(),
                value: json!("allow_once"),
                params: None,
            }],
        }
    }

    #[test]
    fn get_confirmations_returns_pending_acp_permission() {
        let (_tx, rx) = mpsc::channel(1);
        let router = PermissionRouter::new(rx, "test-user".to_owned(), None);
        let (response_tx, _response_rx) = oneshot::channel();

        router.insert_pending_for_test("tool-1".to_owned(), response_tx, sample_confirmation("tool-1"));

        let confirmations = router.get_confirmations();
        assert_eq!(confirmations.len(), 1);
        assert_eq!(confirmations[0].id, "tool-1");
        assert_eq!(confirmations[0].call_id, "tool-1");
        assert_eq!(confirmations[0].description, "Write /tmp/current_time.txt");
    }

    #[test]
    fn confirm_removes_pending_confirmation_and_forwards_option() {
        let (_tx, rx) = mpsc::channel(1);
        let router = PermissionRouter::new(rx, "test-user".to_owned(), None);
        let (response_tx, mut response_rx) = oneshot::channel();
        router.insert_pending_for_test("tool-1".to_owned(), response_tx, sample_confirmation("tool-1"));

        router
            .confirm("tool-1", "allow_once".to_owned(), "conv-1")
            .expect("confirm should succeed");

        assert!(router.get_confirmations().is_empty());
        assert!(matches!(
            response_rx.try_recv(),
            Ok(PermissionDecision::Selected { option_id }) if option_id == "allow_once"
        ));
    }

    #[test]
    fn auto_approve_matches_claude_team_mcp_title_prefix() {
        let request = permission_request_with_title_and_raw_input(
            "mcp__aionui-team__team_members",
            None,
            vec![allow_always_option("allow_always"), reject_option("reject")],
        );

        assert!(is_auto_approve_tool(&request));
    }

    #[test]
    fn auto_approve_matches_codex_raw_input_server_name() {
        let request = permission_request_with_title_and_raw_input(
            "Approve MCP tool call",
            Some(json!({
                "server_name": "aionui-team",
                "request": {
                    "_meta": {
                        "codex_approval_kind": "mcp_tool_call"
                    }
                }
            })),
            vec![
                allow_once_option("approved"),
                allow_always_option("approved-for-session"),
                allow_always_option("approved-always"),
                reject_option("cancel"),
            ],
        );

        assert!(is_auto_approve_tool(&request));
    }

    #[test]
    fn auto_approve_rejects_non_team_mcp_server() {
        let request = permission_request_with_title_and_raw_input(
            "Approve MCP tool call",
            Some(json!({ "server_name": "aionui-image-generation" })),
            vec![allow_always_option("approved-for-session"), reject_option("cancel")],
        );

        assert!(!is_auto_approve_tool(&request));
    }

    #[test]
    fn auto_approve_selects_first_codex_allow_always_option() {
        let request = permission_request_with_title_and_raw_input(
            "Approve MCP tool call",
            Some(json!({ "server_name": "aionui-team" })),
            vec![
                allow_once_option("approved"),
                allow_always_option("approved-for-session"),
                allow_always_option("approved-always"),
                reject_option("cancel"),
            ],
        );

        // `approved-for-session` is selected because it is the first AllowAlways option,
        // not because the option id has special meaning in Dream Core.
        assert_eq!(
            auto_approve_option_id(&request).as_deref(),
            Some("approved-for-session")
        );
    }

    #[test]
    fn auto_approve_selects_claude_allow_always_by_kind() {
        let request = permission_request_with_title_and_raw_input(
            "mcp__aionui-team__team_write_plan",
            None,
            vec![
                allow_always_option("allow_always"),
                allow_once_option("allow"),
                reject_option("reject"),
            ],
        );

        // `allow_always` is selected because it is the only AllowAlways option,
        // not because the option id has special meaning in Dream Core.
        assert_eq!(auto_approve_option_id(&request).as_deref(), Some("allow_always"));
    }

    #[test]
    fn auto_approve_ignores_removed_upgrade_server() {
        let request = permission_request_with_title_and_raw_input(
            concat!("mcp__aionui-team", "-guide__guide_write_plan"),
            None,
            vec![allow_always_option("allow_always"), reject_option("reject")],
        );

        assert_eq!(auto_approve_option_id(&request), None);
    }

    #[test]
    fn auto_approve_selects_first_available_allow_always_option() {
        let request = permission_request_with_title_and_raw_input(
            "Approve MCP tool call",
            Some(json!({ "server_name": "aionui-team" })),
            vec![
                allow_always_option("custom-allow-always"),
                allow_once_option("custom-allow-once"),
            ],
        );

        assert_eq!(auto_approve_option_id(&request).as_deref(), Some("custom-allow-always"));
    }

    #[test]
    fn auto_approve_returns_none_when_team_mcp_has_no_allow_option() {
        let request = permission_request_with_title_and_raw_input(
            "Approve MCP tool call",
            Some(json!({ "server_name": "aionui-team" })),
            vec![reject_option("cancel")],
        );

        assert_eq!(auto_approve_option_id(&request), None);
    }

    #[test]
    fn confirm_missing_permission_returns_specific_error() {
        let (_tx, rx) = mpsc::channel(1);
        let router = PermissionRouter::new(rx, "test-user".to_owned(), None);

        let error = router
            .confirm("missing-tool", "allow_once".to_owned(), "conv-1")
            .expect_err("missing permission should fail");

        assert!(
            error
                .to_string()
                .contains("Pending ACP permission not found: missing-tool")
        );
    }

    #[test]
    fn cancel_all_removes_pending_confirmations() {
        let (_tx, rx) = mpsc::channel(1);
        let router = PermissionRouter::new(rx, "test-user".to_owned(), None);
        let (response_tx, _response_rx) = oneshot::channel();
        router.insert_pending_for_test("tool-1".to_owned(), response_tx, sample_confirmation("tool-1"));

        router.cancel_all();

        assert!(router.get_confirmations().is_empty());
    }

    #[tokio::test]
    async fn start_routes_permission_request_and_exposes_recoverable_confirmation() {
        let (permission_tx, permission_rx) = mpsc::channel(1);
        let router = Arc::new(PermissionRouter::new(permission_rx, "test-user".to_owned(), None));
        let runtime = AgentRuntime::new("conv-1", "/tmp/workspace", 8);
        let mut event_rx = runtime.subscribe();
        router.start(runtime);

        let request = RequestPermissionRequest::new(
            "session-1",
            SdkToolCallUpdate::new(
                "tool-1",
                ToolCallUpdateFields::new()
                    .title("Write file")
                    .kind(SdkToolKind::Edit)
                    .raw_input(json!({ "description": "Write /tmp/current_time.txt" })),
            ),
            vec![PermissionOption::new(
                "allow_once",
                "Allow",
                SdkPermissionOptionKind::AllowOnce,
            )],
        );
        let (response_tx, mut response_rx) = oneshot::channel();

        permission_tx
            .send(PermissionRequest { request, response_tx })
            .await
            .expect("permission request should be accepted");

        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("permission event should be emitted")
            .expect("permission event channel should stay open");
        assert!(matches!(event, AgentStreamEvent::AcpPermission(_)));

        let confirmations = router.get_confirmations();
        assert_eq!(confirmations.len(), 1);
        assert_eq!(confirmations[0].id, "tool-1");
        assert_eq!(confirmations[0].call_id, "tool-1");
        assert_eq!(confirmations[0].command_type.as_deref(), Some("edit"));

        router
            .confirm("tool-1", "allow_once".to_owned(), "conv-1")
            .expect("confirm should resolve routed request");

        assert!(router.get_confirmations().is_empty());
        assert!(matches!(
            response_rx.try_recv(),
            Ok(PermissionDecision::Selected { option_id }) if option_id == "allow_once"
        ));
    }

    #[tokio::test]
    async fn start_auto_approves_team_mcp_with_existing_option_id() {
        let (permission_tx, permission_rx) = mpsc::channel(1);
        let router = Arc::new(PermissionRouter::new(permission_rx, "test-user".to_owned(), None));
        let runtime = AgentRuntime::new("conv-1", "/tmp/workspace", 8);
        router.start(runtime);

        let request = permission_request_with_title_and_raw_input(
            "Approve MCP tool call",
            Some(json!({ "server_name": "aionui-team" })),
            vec![
                allow_once_option("approved"),
                allow_always_option("approved-for-session"),
                reject_option("cancel"),
            ],
        );
        let (response_tx, response_rx) = oneshot::channel();

        permission_tx
            .send(PermissionRequest { request, response_tx })
            .await
            .expect("permission request should be accepted");

        let decision = tokio::time::timeout(Duration::from_secs(1), response_rx)
            .await
            .expect("auto approval should respond")
            .expect("auto approval responder should stay open");

        assert!(matches!(
            decision,
            PermissionDecision::Selected { option_id } if option_id == "approved-for-session"
        ));
        assert!(router.get_confirmations().is_empty());
    }

    // --- Security policy: destructive-command / network-fetch gate ---

    struct MockToolCallSecurityGate {
        verdict: Result<Option<String>, String>,
    }

    #[async_trait::async_trait]
    impl ToolCallSecurityGate for MockToolCallSecurityGate {
        async fn check(
            &self,
            _user_id: &str,
            _command_text: &str,
            _is_network_fetch: bool,
        ) -> Result<Option<String>, String> {
            self.verdict.clone()
        }
    }

    fn router_with_gate(
        permission_rx: mpsc::Receiver<PermissionRequest>,
        gate: MockToolCallSecurityGate,
    ) -> Arc<PermissionRouter> {
        Arc::new(PermissionRouter::new(
            permission_rx,
            "test-user".to_owned(),
            Some(Arc::new(gate)),
        ))
    }

    /// A blocked command must never reach the user for approval, and must
    /// never fall through to auto-approval either — the reject decision is
    /// sent back to the protocol layer directly.
    #[tokio::test]
    async fn start_blocks_a_call_the_security_policy_flags() {
        let (permission_tx, permission_rx) = mpsc::channel(1);
        let router = router_with_gate(
            permission_rx,
            MockToolCallSecurityGate {
                verdict: Ok(Some("matches blocked pattern 'rm -rf'".to_owned())),
            },
        );
        let runtime = AgentRuntime::new("conv-1", "/tmp/workspace", 8);
        let mut event_rx = runtime.subscribe();
        router.start(runtime);

        let request = permission_request_with_title_and_raw_input(
            "Run shell command",
            Some(json!({ "command": "rm -rf /tmp/build" })),
            vec![allow_once_option("allow_once"), reject_option("cancel")],
        );
        let (response_tx, response_rx) = oneshot::channel();

        permission_tx
            .send(PermissionRequest { request, response_tx })
            .await
            .expect("permission request should be accepted");

        let decision = tokio::time::timeout(Duration::from_secs(1), response_rx)
            .await
            .expect("blocked call should respond")
            .expect("responder should stay open");

        assert!(matches!(
            decision,
            PermissionDecision::Selected { option_id } if option_id == "cancel"
        ));
        // No pending confirmation was ever registered — the user was never asked.
        assert!(router.get_confirmations().is_empty());
        // No AcpPermission event reached the UI either.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), event_rx.recv())
                .await
                .is_err()
        );
    }

    /// No reject/cancel option offered: falls back to `Cancelled` rather
    /// than fabricating an option id the caller never actually offered.
    #[tokio::test]
    async fn start_blocks_with_no_reject_option_falls_back_to_cancelled() {
        let (permission_tx, permission_rx) = mpsc::channel(1);
        let router = router_with_gate(
            permission_rx,
            MockToolCallSecurityGate {
                verdict: Ok(Some("blocked".to_owned())),
            },
        );
        let runtime = AgentRuntime::new("conv-1", "/tmp/workspace", 8);
        router.start(runtime);

        let request = permission_request_with_title_and_raw_input(
            "Run shell command",
            Some(json!({ "command": "sudo shutdown now" })),
            vec![allow_once_option("allow_once")],
        );
        let (response_tx, response_rx) = oneshot::channel();

        permission_tx
            .send(PermissionRequest { request, response_tx })
            .await
            .expect("permission request should be accepted");

        let decision = tokio::time::timeout(Duration::from_secs(1), response_rx)
            .await
            .expect("blocked call should respond")
            .expect("responder should stay open");

        assert!(matches!(decision, PermissionDecision::Cancelled));
    }

    /// A failed policy check (DB error, say) fails closed on this one call —
    /// it must not silently let a potentially-destructive command through.
    #[tokio::test]
    async fn start_blocks_when_the_policy_check_itself_fails() {
        let (permission_tx, permission_rx) = mpsc::channel(1);
        let router = router_with_gate(
            permission_rx,
            MockToolCallSecurityGate {
                verdict: Err("platform db unreachable".to_owned()),
            },
        );
        let runtime = AgentRuntime::new("conv-1", "/tmp/workspace", 8);
        router.start(runtime);

        let request = permission_request_with_title_and_raw_input(
            "Run shell command",
            Some(json!({ "command": "ls" })),
            vec![allow_once_option("allow_once"), reject_option("cancel")],
        );
        let (response_tx, response_rx) = oneshot::channel();

        permission_tx
            .send(PermissionRequest { request, response_tx })
            .await
            .expect("permission request should be accepted");

        let decision = tokio::time::timeout(Duration::from_secs(1), response_rx)
            .await
            .expect("failed check should still respond")
            .expect("responder should stay open");

        assert!(matches!(
            decision,
            PermissionDecision::Selected { option_id } if option_id == "cancel"
        ));
    }

    /// A gate that permits the call (`Ok(None)`) must let it proceed through
    /// the normal flow exactly as if no gate were wired at all.
    #[tokio::test]
    async fn start_forwards_to_the_user_when_the_gate_permits_the_call() {
        let (permission_tx, permission_rx) = mpsc::channel(1);
        let router = router_with_gate(permission_rx, MockToolCallSecurityGate { verdict: Ok(None) });
        let runtime = AgentRuntime::new("conv-1", "/tmp/workspace", 8);
        let mut event_rx = runtime.subscribe();
        router.start(runtime);

        let request = permission_request_with_title_and_raw_input(
            "Write file",
            Some(json!({ "path": "/tmp/notes.txt" })),
            vec![allow_once_option("allow_once"), reject_option("cancel")],
        );
        let (response_tx, _response_rx) = oneshot::channel();

        permission_tx
            .send(PermissionRequest { request, response_tx })
            .await
            .expect("permission request should be accepted");

        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("permission event should be emitted")
            .expect("permission event channel should stay open");
        assert!(matches!(event, AgentStreamEvent::AcpPermission(_)));
        assert_eq!(router.get_confirmations().len(), 1);
    }

    /// Records the `is_network_fetch` flag it was called with, so a test can
    /// assert `PermissionRouter` correctly derived it from the request's
    /// ACP `ToolKind` rather than always passing a fixed value.
    struct RecordingToolCallSecurityGate {
        seen: StdMutex<Vec<bool>>,
    }

    #[async_trait::async_trait]
    impl ToolCallSecurityGate for RecordingToolCallSecurityGate {
        async fn check(
            &self,
            _user_id: &str,
            _command_text: &str,
            is_network_fetch: bool,
        ) -> Result<Option<String>, String> {
            self.seen.lock().unwrap().push(is_network_fetch);
            Ok(None)
        }
    }

    fn permission_request_with_kind(kind: SdkToolKind, options: Vec<PermissionOption>) -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            "session-1",
            SdkToolCallUpdate::new(
                "tool-1",
                ToolCallUpdateFields::new().kind(kind).title("Tool call".to_owned()),
            ),
            options,
        )
    }

    /// A `ToolKind::Fetch` call ("retrieving external data" per the ACP
    /// schema) must be reported to the gate as a network fetch.
    #[tokio::test]
    async fn start_flags_a_fetch_tool_call_as_network_fetch() {
        let (permission_tx, permission_rx) = mpsc::channel(1);
        let gate = Arc::new(RecordingToolCallSecurityGate {
            seen: StdMutex::new(Vec::new()),
        });
        let router = Arc::new(PermissionRouter::new(
            permission_rx,
            "test-user".to_owned(),
            Some(gate.clone() as Arc<dyn ToolCallSecurityGate>),
        ));
        let runtime = AgentRuntime::new("conv-1", "/tmp/workspace", 8);
        router.start(runtime);

        let request = permission_request_with_kind(SdkToolKind::Fetch, vec![allow_once_option("allow_once")]);
        let (response_tx, _response_rx) = oneshot::channel();
        permission_tx
            .send(PermissionRequest { request, response_tx })
            .await
            .expect("permission request should be accepted");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(gate.seen.lock().unwrap().as_slice(), &[true]);
    }

    /// A non-fetch call (e.g. editing a file) must be reported as `false`.
    #[tokio::test]
    async fn start_does_not_flag_a_non_fetch_tool_call_as_network_fetch() {
        let (permission_tx, permission_rx) = mpsc::channel(1);
        let gate = Arc::new(RecordingToolCallSecurityGate {
            seen: StdMutex::new(Vec::new()),
        });
        let router = Arc::new(PermissionRouter::new(
            permission_rx,
            "test-user".to_owned(),
            Some(gate.clone() as Arc<dyn ToolCallSecurityGate>),
        ));
        let runtime = AgentRuntime::new("conv-1", "/tmp/workspace", 8);
        router.start(runtime);

        let request = permission_request_with_kind(SdkToolKind::Edit, vec![allow_once_option("allow_once")]);
        let (response_tx, _response_rx) = oneshot::channel();
        permission_tx
            .send(PermissionRequest { request, response_tx })
            .await
            .expect("permission request should be accepted");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(gate.seen.lock().unwrap().as_slice(), &[false]);
    }

    #[test]
    fn extract_command_text_combines_title_and_raw_input() {
        let request = permission_request_with_title_and_raw_input(
            "Run shell command",
            Some(json!({ "command": "rm -rf /tmp" })),
            vec![],
        );
        let text = extract_command_text(&request);
        assert!(text.contains("Run shell command"));
        assert!(text.contains("rm -rf /tmp"));
    }

    #[test]
    fn extract_command_text_handles_no_raw_input() {
        let request = permission_request_with_title_and_raw_input("Write file", None, vec![]);
        let text = extract_command_text(&request);
        assert!(text.contains("Write file"));
    }

    #[test]
    fn select_reject_option_id_prefers_an_offered_reject_option() {
        let request = permission_request_with_title_and_raw_input(
            "x",
            None,
            vec![allow_once_option("allow_once"), reject_option("cancel")],
        );
        assert_eq!(select_reject_option_id(&request).as_deref(), Some("cancel"));
    }

    #[test]
    fn select_reject_option_id_is_none_without_a_reject_option() {
        let request = permission_request_with_title_and_raw_input("x", None, vec![allow_once_option("allow_once")]);
        assert_eq!(select_reject_option_id(&request), None);
    }
}
