//! Cross-session (@@) delivery integration tests — design §4.4 nails.

use std::sync::Arc;

use dream_core_ai_agent::{AgentError, IWorkerTaskManager};
use dream_core_api_types::{ChatFileRef, CreateConversationRequest, ListMessagesQuery, SendMessageRequest};
use dream_core_common::{AgentKillReason, TimestampMs};
use dream_core_conversation::markers::SESSION_MESSAGE_MARKER;
use dream_core_conversation::skill_resolver::SkillResolver;
use dream_core_conversation::{ConversationError, ConversationService};
use dream_core_db::{init_database_memory, SqliteConversationRepository};
use dream_core_realtime::EventBroadcaster;
use serde_json::json;

struct NoopBroadcaster;

impl EventBroadcaster for NoopBroadcaster {
    fn broadcast(&self, _event: dream_core_api_types::WebSocketMessage<serde_json::Value>) {}
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

const USER: &str = "system_default_user";

fn workspace_extra() -> serde_json::Value {
    let workspace = std::env::temp_dir().join("one-session-delivery-test");
    std::fs::create_dir_all(&workspace).ok();
    json!({ "workspace": workspace.to_string_lossy() })
}

fn make_create_req() -> CreateConversationRequest {
    serde_json::from_value(json!({
        "type": "acp",
        "extra": workspace_extra()
    }))
    .unwrap()
}

async fn setup() -> (ConversationService, Arc<dyn IWorkerTaskManager>) {
    let db = init_database_memory().await.unwrap();
    let repo = Arc::new(SqliteConversationRepository::new(db.pool().clone()));
    let agent_metadata_repo: Arc<dyn dream_core_db::IAgentMetadataRepository> =
        Arc::new(dream_core_db::SqliteAgentMetadataRepository::new(db.pool().clone()));
    let acp_session_repo: Arc<dyn dream_core_db::IAcpSessionRepository> =
        Arc::new(dream_core_db::SqliteAcpSessionRepository::new(db.pool().clone()));
    let task_mgr: Arc<dyn IWorkerTaskManager> = Arc::new(NoopTaskManager);
    let svc = ConversationService::new(
        std::env::temp_dir(),
        Arc::new(NoopBroadcaster),
        Arc::new(EmptySkillResolver),
        task_mgr.clone(),
        repo,
        agent_metadata_repo,
        acp_session_repo,
    );
    (svc, task_mgr)
}

fn send_req(content: &str) -> SendMessageRequest {
    SendMessageRequest {
        content: content.into(),
        files: Vec::<ChatFileRef>::new(),
        inject_skills: Vec::new(),
        hidden: false,
        reply_requested: false,
    }
}

#[tokio::test]
async fn cross_session_send_rejects_team_owned_target() {
    let (svc, task_mgr) = setup().await;
    let from = svc.create(USER, make_create_req()).await.unwrap();
    let mut team_req = make_create_req();
    team_req.extra = json!({
        "workspace": workspace_extra()["workspace"],
        "teamId": "team_abc"
    });
    let team_conv = svc.create(USER, team_req).await.unwrap();

    let content = format!("ping @@conv:{}", team_conv.id);
    let err = svc
        .send_message(USER, &from.id, send_req(&content), &task_mgr)
        .await
        .unwrap_err();

    assert!(
        matches!(err, ConversationError::Forbidden { .. }),
        "expected team target to be forbidden, got {err:?}"
    );
}

#[tokio::test]
async fn send_message_rate_limit_before_bad_target_reference() {
    const OUTBOUND_MAX: u32 = 20;
    let (svc, task_mgr) = setup().await;
    let from = svc.create(USER, make_create_req()).await.unwrap();
    let mut target_ids = Vec::new();
    for _ in 0..OUTBOUND_MAX {
        target_ids.push(svc.create(USER, make_create_req()).await.unwrap().id);
    }
    for (i, to) in target_ids.iter().enumerate() {
        let content = format!("burst {i} @@conv:{to}");
        svc.send_message(USER, &from.id, send_req(&content), &task_mgr)
            .await
            .unwrap();
    }
    let err = svc
        .send_message(
            USER,
            &from.id,
            send_req("one more @@conv:definitely_not_a_real_id"),
            &task_mgr,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ConversationError::PolicyDenied {
                code: "SESSION_DELIVERY_RATE_LIMIT",
                ..
            }
        ),
        "expected rate limit, not target validation: {err:?}"
    );
}

#[tokio::test]
async fn drainer_delivers_user_message_to_target_conversation() {
    let (svc, task_mgr) = setup().await;
    let a = svc.create(USER, make_create_req()).await.unwrap();
    let b = svc.create(USER, make_create_req()).await.unwrap();

    let content = format!("deliver this @@conv:{}", b.id);
    svc.send_message(USER, &a.id, send_req(&content), &task_mgr)
        .await
        .unwrap();

    assert!(svc.session_delivery_hub().pending_len() > 0);

    svc.run_session_delivery_tick(&task_mgr).await;

    let messages = svc
        .list_messages(
            USER,
            &b.id,
            ListMessagesQuery {
                limit: Some(20),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let joined: String = messages
        .items
        .iter()
        .filter_map(|m| {
            m.content
                .get("content")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined.contains(SESSION_MESSAGE_MARKER),
        "expected inbound session block in target history, got: {joined}"
    );
    assert!(joined.contains("deliver this"));
}
