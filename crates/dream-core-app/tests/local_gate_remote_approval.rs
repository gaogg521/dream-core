//! C0-1 plan A: the personal build's tool-call gate drives the company's own
//! workflow API over the upstream channel. These cover the three outcomes the
//! member can see (approved / rejected with note / timed out) plus the
//! fail-closed posture when the channel was never synced.

use std::sync::Arc;

use axum::Json;
use axum::routing::{get, post};
use dream_core_ai_agent::ToolCallSecurityGate;
use dream_core_system::{EnterpriseUpstream, EnterpriseUpstreamService, ToolSecurityPolicy, ToolSecurityService};
use serde_json::json;

/// What the fake company server recorded, so assertions can cover the create
/// request itself (kind, expiry) and not just the final verdict.
#[derive(Default, Clone)]
struct Seen {
    create_body: serde_json::Value,
    auth: String,
}

type PollState = (
    Arc<std::sync::Mutex<Seen>>,
    &'static str,
    &'static str,
    Arc<std::sync::atomic::AtomicUsize>,
);

/// A stand-in for the company server. `decision` is what the created task
/// reports from its second poll onward (the first poll stays `pending`, so
/// the test also proves the loop actually loops).
async fn spawn_company_server(decision: &'static str, note: &'static str, seen: Arc<std::sync::Mutex<Seen>>) -> String {
    async fn create(
        axum::extract::State(seen): axum::extract::State<Arc<std::sync::Mutex<Seen>>>,
        headers: axum::http::HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        {
            let mut slot = seen.lock().unwrap();
            slot.create_body = body.clone();
            slot.auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned();
        }
        Json(json!({ "success": true, "data": { "id": "task-1", "status": "pending" } }))
    }

    async fn get_task(
        axum::extract::State((seen, decision, note, polls)): axum::extract::State<PollState>,
        axum::extract::Path(id): axum::extract::Path<String>,
    ) -> Json<serde_json::Value> {
        let _ = &seen;
        assert_eq!(id, "task-1");
        let n = polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let status = if n == 0 { "pending" } else { decision };
        Json(json!({
            "success": true,
            "data": { "id": id, "status": status, "note": note },
        }))
    }

    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let polls_for_state = polls.clone();
    let app = axum::Router::new()
        .route("/api/workflow/tasks", post(create).with_state(seen.clone()))
        .route(
            "/api/workflow/tasks/{id}",
            get(get_task).with_state((seen, decision, note, polls_for_state)),
        )
        .with_state(());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// The gate under test, with a policy that demands terminal approval.
fn gate(upstream: Arc<EnterpriseUpstreamService>) -> dream_core_app::LocalToolSecurityGate {
    dream_core_app::LocalToolSecurityGate {
        policy: Arc::new(ToolSecurityService::new()),
        upstream,
        events: None,
        conversations: None,
    }
}

fn approval_required_policy() -> ToolSecurityPolicy {
    ToolSecurityPolicy {
        terminal_tools_require_approval: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn approved_decision_releases_the_call() {
    let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
    let base = spawn_company_server("approved", "", seen.clone()).await;
    let upstream = Arc::new(EnterpriseUpstreamService::new());
    upstream.set(EnterpriseUpstream {
        base_url: base,
        token: "member-token".to_owned(),
    });
    let gate = gate(upstream);
    gate.policy.set_policy(approval_required_policy());

    let verdict = gate
        .check("member-1", "ExecCommand {\"cmd\":\"echo hi\"}", false, true)
        .await;

    assert_eq!(verdict, Ok(None), "an approved call must proceed");
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.auth, "Bearer member-token",
        "the member's token must authenticate the call"
    );
    assert_eq!(seen.create_body["kind"], "tool");
    assert_eq!(
        seen.create_body["payload"]["commandText"],
        "ExecCommand {\"cmd\":\"echo hi\"}"
    );
    assert!(
        seen.create_body["expiresAtMs"].is_i64(),
        "the task must carry an expiry so the queue reads truthfully after the deadline"
    );
}

#[tokio::test]
async fn rejected_decision_denies_with_the_note() {
    let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
    let base = spawn_company_server("rejected", "production machine", seen.clone()).await;
    let upstream = Arc::new(EnterpriseUpstreamService::new());
    upstream.set(EnterpriseUpstream {
        base_url: base,
        token: "t".to_owned(),
    });
    let gate = gate(upstream);
    gate.policy.set_policy(approval_required_policy());

    let verdict = gate.check("member-1", "ExecCommand {}", false, true).await;

    assert_eq!(
        verdict,
        Ok(Some(
            "blocked by company security policy (rejected by an administrator: production machine)".to_owned()
        ))
    );
}

#[tokio::test]
async fn no_channel_fails_closed() {
    let gate = gate(Arc::new(EnterpriseUpstreamService::new()));
    gate.policy.set_policy(approval_required_policy());

    let verdict = gate.check("member-1", "ExecCommand {}", false, true).await;

    assert!(
        verdict.is_err(),
        "policy demands approval but no channel was synced — fail closed, never a silent allow"
    );
}

#[tokio::test]
async fn permissive_policy_never_touches_the_channel() {
    let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
    let base = spawn_company_server("approved", "", seen.clone()).await;
    let upstream = Arc::new(EnterpriseUpstreamService::new());
    upstream.set(EnterpriseUpstream {
        base_url: base,
        token: "t".to_owned(),
    });
    let gate = gate(upstream);
    // No policy set at all — the personal-edition default posture.

    let verdict = gate.check("member-1", "ExecCommand {}", false, true).await;

    assert_eq!(verdict, Ok(None), "an empty policy allows everything and costs nothing");
    assert!(
        seen.lock().unwrap().create_body.is_null(),
        "no approval task may be created"
    );
}

#[tokio::test]
async fn non_terminal_tool_skips_the_approval_even_when_demanded() {
    let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
    let base = spawn_company_server("approved", "", seen.clone()).await;
    let upstream = Arc::new(EnterpriseUpstreamService::new());
    upstream.set(EnterpriseUpstream {
        base_url: base,
        token: "t".to_owned(),
    });
    let gate = gate(upstream);
    gate.policy.set_policy(approval_required_policy());

    let verdict = gate.check("member-1", "Read {}", false, false).await;

    assert_eq!(verdict, Ok(None), "approval gates terminal tools only");
    assert!(
        seen.lock().unwrap().create_body.is_null(),
        "no approval task may be created"
    );
}

#[tokio::test]
async fn approval_request_is_titled_with_the_conversation_and_reads_the_command() {
    let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
    let base = spawn_company_server("approved", "", seen.clone()).await;
    let upstream = Arc::new(EnterpriseUpstreamService::new());
    upstream.set(EnterpriseUpstream {
        base_url: base,
        token: "t".to_owned(),
    });
    let gate = gate(upstream);
    gate.policy.set_policy(approval_required_policy());
    let context = dream_core_ai_agent::ToolCallContext {
        conversation_id: Some("conv-1".to_owned()),
        conversation_title: Some("My test chat".to_owned()),
        tool_name: Some("ExecCommand".to_owned()),
        tool_input: Some(serde_json::json!({ "cmd": "Get-ChildItem -Force" })),
    };

    let verdict = gate
        .check_in_context(
            "member-1",
            "ExecCommand {\"cmd\":\"Get-ChildItem -Force\"}",
            false,
            true,
            &context,
        )
        .await;

    assert_eq!(verdict, Ok(None));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.create_body["title"], "My test chat");
    let detail = seen.create_body["detail"].as_str().unwrap();
    assert!(detail.contains("Get-ChildItem -Force"), "detail: {detail}");
    assert!(
        !detail.contains("\"cmd\""),
        "the raw JSON must not be the headline: {detail}"
    );
    assert_eq!(seen.create_body["payload"]["conversationId"], "conv-1");
    assert_eq!(seen.create_body["payload"]["command"], "Get-ChildItem -Force");
}

/// A company server whose current policy no longer asks for approval, counting
/// how many approval tasks anyone tried to create on it.
async fn spawn_relaxed_company_server(creates: Arc<std::sync::atomic::AtomicUsize>) -> String {
    async fn policy() -> Json<serde_json::Value> {
        Json(json!({ "success": true, "data": { "terminalToolsRequireApproval": false } }))
    }
    async fn create(
        axum::extract::State(creates): axum::extract::State<Arc<std::sync::atomic::AtomicUsize>>,
    ) -> Json<serde_json::Value> {
        creates.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Json(json!({ "success": true, "data": { "id": "task-x", "status": "pending" } }))
    }
    let app = axum::Router::new()
        .route("/api/one/platform/my-security-policy", get(policy))
        .route("/api/workflow/tasks", post(create).with_state(creates));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_stale_local_policy_does_not_hold_the_call_once_the_company_relaxed_it() {
    let creates = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let base = spawn_relaxed_company_server(creates.clone()).await;
    let upstream = Arc::new(EnterpriseUpstreamService::new());
    upstream.set(EnterpriseUpstream {
        base_url: base,
        token: "t".to_owned(),
    });
    let gate = gate(upstream);
    // The local copy still says "approval required" — the sync has not run yet.
    gate.policy.set_policy(approval_required_policy());

    let verdict = gate
        .check("member-1", "ExecCommand {\"cmd\":\"dir\"}", false, true)
        .await;

    assert_eq!(verdict, Ok(None), "the company no longer requires approval");
    assert_eq!(
        creates.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no approval task may be created for a policy the company already relaxed"
    );
    assert!(
        !gate.policy.policy().terminal_tools_require_approval,
        "the stale local copy is corrected so later calls skip the lookup"
    );
}

#[tokio::test]
async fn the_conversation_is_told_it_is_waiting_and_then_how_it_ended() {
    let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
    let base = spawn_company_server("approved", "", seen.clone()).await;
    let upstream = Arc::new(EnterpriseUpstreamService::new());
    upstream.set(EnterpriseUpstream {
        base_url: base,
        token: "t".to_owned(),
    });
    let bus = Arc::new(dream_core_realtime::BroadcastEventBus::new(16));
    let mut events = bus.subscribe();
    let mut gate = gate(upstream);
    gate.events = Some(bus.clone());
    gate.policy.set_policy(approval_required_policy());
    let context = dream_core_ai_agent::ToolCallContext {
        conversation_id: Some("conv-9".to_owned()),
        tool_name: Some("ExecCommand".to_owned()),
        tool_input: Some(serde_json::json!({ "cmd": "Get-ChildItem" })),
        ..Default::default()
    };

    let verdict = gate
        .check_in_context("member-1", "ExecCommand {}", false, true, &context)
        .await;
    assert_eq!(verdict, Ok(None));

    let pending = events.try_recv().expect("a pending event must be broadcast");
    assert_eq!(pending.name, "conversation.approvalPending");
    assert_eq!(pending.data["conversationId"], "conv-9");
    assert_eq!(pending.data["command"], "Get-ChildItem");
    assert!(pending.data["expiresAtMs"].is_i64());

    let resolved = events.try_recv().expect("a resolved event must follow");
    assert_eq!(resolved.name, "conversation.approvalResolved");
    assert_eq!(resolved.data["conversationId"], "conv-9");
    assert_eq!(resolved.data["outcome"], "approved");
}

#[tokio::test]
async fn a_call_without_a_conversation_announces_nothing() {
    let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
    let base = spawn_company_server("approved", "", seen.clone()).await;
    let upstream = Arc::new(EnterpriseUpstreamService::new());
    upstream.set(EnterpriseUpstream {
        base_url: base,
        token: "t".to_owned(),
    });
    let bus = Arc::new(dream_core_realtime::BroadcastEventBus::new(16));
    let mut events = bus.subscribe();
    let mut gate = gate(upstream);
    gate.events = Some(bus.clone());
    gate.policy.set_policy(approval_required_policy());

    let verdict = gate.check("member-1", "ExecCommand {}", false, true).await;

    assert_eq!(verdict, Ok(None));
    assert!(events.try_recv().is_err(), "no conversation, nobody to tell");
}

/// The ACP path knows only the conversation's id; the gate names it from the
/// member's own conversation, as it is called right now.
#[tokio::test]
async fn a_call_known_only_by_conversation_id_is_titled_with_the_conversations_name() {
    use dream_core_db::models::ConversationRow;
    use dream_core_db::{IConversationRepository, SqliteConversationRepository};

    let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
    let base = spawn_company_server("approved", "", seen.clone()).await;
    let upstream = Arc::new(EnterpriseUpstreamService::new());
    upstream.set(EnterpriseUpstream {
        base_url: base,
        token: "t".to_owned(),
    });

    let db = dream_core_db::init_database_memory().await.unwrap();
    sqlx::query(
        "INSERT INTO users (id, user_type, username, password_hash, status, session_generation, created_at, updated_at)          VALUES ('member-1', 'local', 'member-1', 'hash', 'active', 0, 0, 0)",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let repo = Arc::new(SqliteConversationRepository::new(db.pool().clone()));
    let now = dream_core_common::now_ms();
    repo.create(&ConversationRow {
        id: "conv-acp".to_owned(),
        user_id: "member-1".to_owned(),
        name: "Quarterly report chat".to_owned(),
        r#type: "acp".to_owned(),
        extra: "{}".to_owned(),
        model: None,
        status: Some("finished".to_owned()),
        source: Some("dream".to_owned()),
        channel_chat_id: None,
        pinned: false,
        pinned_at: None,
        created_at: now,
        updated_at: now,
        project_id: None,
        folder_id: None,
        name_source: None,
    })
    .await
    .unwrap();

    let mut gate = gate(upstream);
    gate.conversations = Some(repo);
    gate.policy.set_policy(approval_required_policy());
    let context = dream_core_ai_agent::ToolCallContext {
        conversation_id: Some("conv-acp".to_owned()),
        tool_input: Some(serde_json::json!({ "command": "dir" })),
        ..Default::default()
    };

    let verdict = gate
        .check_in_context(
            "member-1",
            "Run shell command {\"command\":\"dir\"}",
            false,
            true,
            &context,
        )
        .await;

    assert_eq!(verdict, Ok(None));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.create_body["title"], "Quarterly report chat");
    assert_eq!(
        seen.create_body["payload"]["conversationTitle"],
        "Quarterly report chat"
    );
}
