//! Import-shared acceptance: a conversation snapshot (the shape the
//! enterprise share read and the personal SESSION_SHARE block both carry)
//! imports as a NEW conversation owned by the caller, messages intact, no
//! model attached — the importer picks one on first send.

use std::sync::Arc;

use dream_core_ai_agent::{AgentError, IWorkerTaskManager};
use dream_core_api_types::{
    ImportSharedConversationRequest, ImportSharedMessage, ListConversationsQuery, ListMessagesQuery, SendMessageRequest,
};
use dream_core_common::{AgentKillReason, TimestampMs};
use dream_core_conversation::skill_resolver::SkillResolver;
use dream_core_conversation::{ConversationError, ConversationService};
use dream_core_db::{SqliteConversationRepository, init_database_memory};
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

#[tokio::test]
async fn import_creates_new_conversation_with_snapshot_messages() {
    let (svc, task_mgr) = setup().await;

    let req = ImportSharedConversationRequest {
        name: "导入的会话副本".into(),
        messages: vec![
            ImportSharedMessage {
                message_type: "text".into(),
                content: json!({ "content": "第一条" }).to_string(),
                position: Some("right".into()),
                created_at: Some(1000),
            },
            ImportSharedMessage {
                message_type: "text".into(),
                content: json!({ "content": "第二条" }).to_string(),
                position: Some("left".into()),
                created_at: Some(2000),
            },
        ],
    };
    let response = svc.import_shared_conversation(USER, req).await.unwrap();
    assert_eq!(response.imported_messages, 2);

    let page = svc
        .list_messages(
            USER,
            &response.conversation_id,
            ListMessagesQuery {
                limit: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let texts: Vec<String> = page
        .items
        .iter()
        .filter_map(|m| m.content.get("content").and_then(|v| v.as_str()).map(str::to_owned))
        .collect();
    assert_eq!(texts, vec!["第一条".to_owned(), "第二条".to_owned()]);

    // The copy is owned by the importer: it appears in their own list with no
    // model, and sending to it behaves like an ordinary conversation send.
    let list = svc
        .list(
            USER,
            ListConversationsQuery {
                limit: Some(10),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let imported = list.items.iter().find(|c| c.id == response.conversation_id).unwrap();
    assert!(imported.model.is_none(), "imported copy must carry no model");
    let send = svc
        .send_message(
            USER,
            &response.conversation_id,
            SendMessageRequest {
                content: "continue here".into(),
                files: vec![],
                inject_skills: vec![],
                hidden: false,
                reply_requested: false,
            },
            &task_mgr,
        )
        .await;
    // Sending is allowed regardless of whether an agent turn can start; the
    // failure mode we are pinning is NOT "cannot send to the imported copy".
    assert!(
        send.is_ok() || matches!(send, Err(ConversationError::BadRequest { .. })),
        "send to imported copy should not be forbidden"
    );
}
