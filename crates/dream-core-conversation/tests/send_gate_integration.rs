//! T3: `SendGate` is now checked inside `ConversationService::send_message`
//! and `run_agent_turn` themselves, not just at the HTTP `send_msg` handler —
//! see `dream-core-conversation/src/service.rs`'s two call sites and
//! `delivery-gaps` T3 for why (cron and the IM-channel `MessageService` call
//! these methods directly and used to bypass the gate entirely).
//!
//! These tests exercise the gate at the `ConversationService` level with a
//! minimal harness (mirrors `tests/active_lease.rs`'s `setup()`), not through
//! HTTP or a real billing plane — the billing-side policy logic itself is
//! `dream-core-app`'s concern and already covered there.

use std::sync::Arc;

use dream_core_ai_agent::{AgentError, IWorkerTaskManager};
use dream_core_api_types::{ChatFileRef, CreateConversationRequest, SendMessageRequest, WebSocketMessage};
use dream_core_common::{AgentKillReason, TimestampMs};
use dream_core_conversation::skill_resolver::SkillResolver;
use dream_core_conversation::{
    ConversationAgentTurnRequest, ConversationError, ConversationService, PolicyDenial, SendGate,
};
use dream_core_db::init_database_memory;
use dream_core_realtime::EventBroadcaster;
use serde_json::json;

struct NoopBroadcaster;

impl EventBroadcaster for NoopBroadcaster {
    fn broadcast(&self, _event: WebSocketMessage<serde_json::Value>) {}
}

struct NoopTaskManager;

#[async_trait::async_trait]
impl IWorkerTaskManager for NoopTaskManager {
    fn get_task(&self, _: &str) -> Option<dream_core_ai_agent::AgentInstance> {
        None
    }

    async fn get_or_build_task(
        &self,
        _: &str,
        _: dream_core_ai_agent::types::BuildTaskOptions,
    ) -> Result<dream_core_ai_agent::AgentInstance, AgentError> {
        Err(AgentError::internal("noop"))
    }

    fn kill(&self, _: &str, _: Option<AgentKillReason>) -> Result<(), AgentError> {
        Ok(())
    }

    fn kill_and_wait(
        &self,
        _: &str,
        _: Option<AgentKillReason>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(std::future::ready(()))
    }

    async fn clear(&self) {}

    fn active_count(&self) -> usize {
        0
    }

    fn collect_idle(&self, _: TimestampMs) -> Vec<String> {
        vec![]
    }
}

struct EmptySkillResolver;

#[async_trait::async_trait]
impl SkillResolver for EmptySkillResolver {
    async fn auto_inject_names(&self) -> Vec<String> {
        Vec::new()
    }

    async fn resolve_skills(&self, _names: &[String]) -> Vec<dream_core_extension::ResolvedAgentSkill> {
        Vec::new()
    }

    async fn link_workspace_skills(
        &self,
        _workspace: &std::path::Path,
        _rel_dirs: &[&str],
        _skills: &[dream_core_extension::ResolvedAgentSkill],
    ) -> usize {
        0
    }
}

const USER_ID: &str = "system_default_user";

/// Always refuses — stands in for a team over its spend budget.
struct RejectingSendGate;

#[async_trait::async_trait]
impl SendGate for RejectingSendGate {
    async fn check_send(&self, _user_id: &str, _model: Option<&str>) -> Result<(), PolicyDenial> {
        Err(PolicyDenial::new(
            "BUDGET_EXCEEDED",
            "the team's monthly spend budget is exhausted",
        ))
    }

    async fn check_model(&self, _user_id: &str, _model: &str) -> Result<(), PolicyDenial> {
        Ok(())
    }
}

/// Always allows — proves a wired-but-permissive gate doesn't change behavior.
struct AllowingSendGate;

#[async_trait::async_trait]
impl SendGate for AllowingSendGate {
    async fn check_send(&self, _user_id: &str, _model: Option<&str>) -> Result<(), PolicyDenial> {
        Ok(())
    }

    async fn check_model(&self, _user_id: &str, _model: &str) -> Result<(), PolicyDenial> {
        Ok(())
    }
}

async fn setup() -> ConversationService {
    let db = init_database_memory().await.unwrap();
    let repo = Arc::new(dream_core_db::SqliteConversationRepository::new(db.pool().clone()));
    let agent_metadata_repo: Arc<dyn dream_core_db::IAgentMetadataRepository> =
        Arc::new(dream_core_db::SqliteAgentMetadataRepository::new(db.pool().clone()));
    let acp_session_repo: Arc<dyn dream_core_db::IAcpSessionRepository> =
        Arc::new(dream_core_db::SqliteAcpSessionRepository::new(db.pool().clone()));

    ConversationService::new(
        std::env::temp_dir(),
        Arc::new(NoopBroadcaster),
        Arc::new(EmptySkillResolver),
        Arc::new(NoopTaskManager),
        repo,
        agent_metadata_repo,
        acp_session_repo,
    )
}

fn make_send_req() -> SendMessageRequest {
    SendMessageRequest {
        content: "hello".into(),
        files: Vec::<ChatFileRef>::new(),
        inject_skills: Vec::new(),
        hidden: false,
    }
}

fn make_turn_req(conversation_id: &str) -> ConversationAgentTurnRequest {
    ConversationAgentTurnRequest {
        user_id: USER_ID.into(),
        conversation_id: conversation_id.into(),
        content: "hello".into(),
        files: vec![],
        inject_skills: vec![],
        required_runtime_mode: None,
        persist_user_message: true,
        user_message_hidden: false,
        on_started: None,
    }
}

// ── send_message (HTTP send_msg + IM-channel send_to_agent's shared entry) ──

#[tokio::test]
async fn send_message_without_a_gate_is_unaffected() {
    let service = setup().await;
    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(NoopTaskManager);

    // No `with_send_gate` call at all -- the personal-edition / no-company
    // case. Must fall straight through to the ordinary conversation lookup
    // (which fails for a made-up id), never a policy denial.
    let err = service
        .send_message(USER_ID, "does-not-exist", make_send_req(), &task_manager)
        .await
        .unwrap_err();

    assert!(
        matches!(err, ConversationError::NotFound { .. }),
        "expected the ordinary not-found path, got {err:?}"
    );
}

#[tokio::test]
async fn send_message_is_blocked_by_a_rejecting_gate() {
    let service = setup().await;
    service.with_send_gate(Arc::new(RejectingSendGate));
    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(NoopTaskManager);

    // Even a conversation id that doesn't exist proves the point: the gate
    // must run BEFORE the conversation lookup, not after.
    let err = service
        .send_message(USER_ID, "does-not-exist", make_send_req(), &task_manager)
        .await
        .unwrap_err();

    match err {
        ConversationError::PolicyDenied { code, message, .. } => {
            assert_eq!(code, "BUDGET_EXCEEDED");
            assert!(message.contains("budget"));
        }
        other => panic!("expected PolicyDenied, got {other:?}"),
    }
}

#[tokio::test]
async fn send_message_passes_through_an_allowing_gate() {
    let service = setup().await;
    service.with_send_gate(Arc::new(AllowingSendGate));
    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(NoopTaskManager);

    let err = service
        .send_message(USER_ID, "does-not-exist", make_send_req(), &task_manager)
        .await
        .unwrap_err();

    assert!(
        matches!(err, ConversationError::NotFound { .. }),
        "an Ok gate must not change the outcome, got {err:?}"
    );
}

// ── run_agent_turn (cron's shared entry) ──

#[tokio::test]
async fn run_agent_turn_without_a_gate_is_unaffected() {
    let service = setup().await;

    let err = service
        .run_agent_turn(make_turn_req("does-not-exist"))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ConversationError::NotFound { .. }),
        "expected the ordinary not-found path, got {err:?}"
    );
}

#[tokio::test]
async fn run_agent_turn_is_blocked_by_a_rejecting_gate() {
    let service = setup().await;
    service.with_send_gate(Arc::new(RejectingSendGate));

    let err = service
        .run_agent_turn(make_turn_req("does-not-exist"))
        .await
        .unwrap_err();

    match err {
        ConversationError::PolicyDenied { code, .. } => assert_eq!(code, "BUDGET_EXCEEDED"),
        other => panic!("expected PolicyDenied, got {other:?}"),
    }
}

#[tokio::test]
async fn run_agent_turn_passes_through_an_allowing_gate() {
    let service = setup().await;
    service.with_send_gate(Arc::new(AllowingSendGate));

    let err = service
        .run_agent_turn(make_turn_req("does-not-exist"))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ConversationError::NotFound { .. }),
        "an Ok gate must not change the outcome, got {err:?}"
    );
}

#[tokio::test]
async fn run_agent_turn_rejects_before_touching_a_real_conversation() {
    // Stronger version of the two tests above: use a REAL conversation (one
    // that would otherwise succeed past the lookup) to prove the gate check
    // truly runs first, not just that a missing id happens to also be an
    // error.
    let service = setup().await;
    let conversation = service
        .create(
            USER_ID,
            serde_json::from_value::<CreateConversationRequest>(json!({
                "type": "acp",
                "extra": { "workspace": std::env::temp_dir().to_string_lossy() }
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    service.with_send_gate(Arc::new(RejectingSendGate));

    let err = service
        .run_agent_turn(make_turn_req(&conversation.id))
        .await
        .unwrap_err();

    match err {
        ConversationError::PolicyDenied { code, .. } => assert_eq!(code, "BUDGET_EXCEEDED"),
        other => panic!("expected PolicyDenied even for a real, reachable conversation, got {other:?}"),
    }
}
