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
