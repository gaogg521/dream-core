use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use dream_core_ai_agent::agent_task::{AgentInstance, IAgentTask};
use dream_core_ai_agent::protocol::events::FinishEventData;
use dream_core_ai_agent::types::{BuildTaskOptions, SendMessageData};
use dream_core_ai_agent::{AgentError, AgentSendError, AgentStreamEvent, IMockAgent, IWorkerTaskManager};
use dream_core_api_types::WebSocketMessage;
use dream_core_channel::channel_settings::ChannelSettingsService;
use dream_core_channel::error::ChannelError;
use dream_core_channel::message_service::ChannelMessageService;
use dream_core_channel::types::PluginType;
use dream_core_common::{AgentKillReason, AgentType, ConversationStatus, TimestampMs};
use dream_core_conversation::ConversationService;
use dream_core_conversation::skill_resolver::{ResolvedAgentSkill, SkillResolver};
use dream_core_db::models::AssistantSessionRow;
use dream_core_db::models::UpsertAssistantDefinitionParams;
use dream_core_db::{
    IAcpSessionRepository, IAssistantDefinitionRepository, IClientPreferenceRepository, IConversationRepository,
    SqliteAcpSessionRepository, SqliteAgentMetadataRepository, SqliteAssistantDefinitionRepository,
    SqliteAssistantOverlayRepository, SqliteAssistantPreferenceRepository, SqliteClientPreferenceRepository,
    SqliteConversationRepository, init_database_memory,
};
use dream_core_realtime::EventBroadcaster;
use tokio::sync::broadcast;

const TEST_OWNER_USER_ID: &str = "system_default_user";

#[tokio::test]
async fn missing_channel_model_fails_before_conversation_or_agent_creation() {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();
    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(RecordingTaskManager::new());
    let service = Arc::new(ConversationService::new(
        std::env::temp_dir(),
        Arc::new(TestBroadcaster::new()),
        Arc::new(NoopSkillResolver),
        Arc::clone(&task_manager),
        Arc::new(SqliteConversationRepository::new(pool.clone())),
        Arc::new(SqliteAgentMetadataRepository::new(pool.clone())),
        Arc::new(SqliteAcpSessionRepository::new(pool.clone())),
    ));
    let settings = Arc::new(ChannelSettingsService::new(Arc::new(
        SqliteClientPreferenceRepository::new(pool),
    )));
    let channel = ChannelMessageService::new(service, Arc::clone(&task_manager), settings);
    let session = AssistantSessionRow {
        id: "missing-model-session".into(),
        user_id: "channel-user".into(),
        agent_type: "dream".into(),
        conversation_id: None,
        workspace: None,
        chat_id: Some("test-chat".into()),
        created_at: 1,
        last_activity: 1,
    };
    let result = channel
        .send_to_agent(TEST_OWNER_USER_ID, &session, "test", PluginType::Dingtalk)
        .await;
    assert!(matches!(result, Err(ChannelError::InvalidConfig(message)) if message.contains("默认模型")));
}

struct TestBroadcaster {
    events: Mutex<Vec<WebSocketMessage<serde_json::Value>>>,
}

impl TestBroadcaster {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }
}

impl EventBroadcaster for TestBroadcaster {
    fn broadcast(&self, event: WebSocketMessage<serde_json::Value>) {
        self.events.lock().unwrap().push(event);
    }
}

struct NoopSkillResolver;

#[async_trait]
impl SkillResolver for NoopSkillResolver {
    async fn auto_inject_names(&self) -> Vec<String> {
        Vec::new()
    }

    async fn resolve_skills(&self, _names: &[String]) -> Vec<ResolvedAgentSkill> {
        Vec::new()
    }

    async fn link_workspace_skills(
        &self,
        _workspace: &std::path::Path,
        _rel_dirs: &[&str],
        _skills: &[ResolvedAgentSkill],
    ) -> usize {
        0
    }
}

struct ScriptedAgent {
    conversation_id: String,
    event_tx: broadcast::Sender<AgentStreamEvent>,
    sent: Arc<Mutex<Vec<String>>>,
    workspace: String,
}

impl ScriptedAgent {
    fn new(conversation_id: &str, sent: Arc<Mutex<Vec<String>>>) -> Self {
        let (event_tx, _) = broadcast::channel(16);
        Self {
            conversation_id: conversation_id.to_owned(),
            event_tx,
            sent,
            workspace: std::env::temp_dir().to_string_lossy().into_owned(),
        }
    }
}

#[async_trait]
impl IAgentTask for ScriptedAgent {
    fn agent_type(&self) -> AgentType {
        AgentType::DreamEngine
    }

    fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    fn workspace(&self) -> &str {
        &self.workspace
    }

    fn status(&self) -> Option<ConversationStatus> {
        Some(ConversationStatus::Finished)
    }

    fn last_activity_at(&self) -> TimestampMs {
        0
    }

    fn subscribe(&self) -> broadcast::Receiver<AgentStreamEvent> {
        self.event_tx.subscribe()
    }

    async fn send_message(&self, data: SendMessageData) -> Result<(), AgentSendError> {
        self.sent.lock().unwrap().push(data.content);
        let _ = self.event_tx.send(AgentStreamEvent::Finish(FinishEventData::default()));
        Ok(())
    }

    async fn cancel(&self) -> Result<(), AgentError> {
        Ok(())
    }

    fn kill(&self, _reason: Option<AgentKillReason>) -> Result<(), AgentError> {
        Ok(())
    }
}

impl IMockAgent for ScriptedAgent {}

struct RecordingTaskManager {
    agents: Mutex<std::collections::HashMap<String, AgentInstance>>,
    sent: Arc<Mutex<Vec<String>>>,
}

impl RecordingTaskManager {
    fn new() -> Self {
        Self {
            agents: Mutex::new(std::collections::HashMap::new()),
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl IWorkerTaskManager for RecordingTaskManager {
    fn get_task(&self, conversation_id: &str) -> Option<AgentInstance> {
        self.agents.lock().unwrap().get(conversation_id).cloned()
    }

    async fn get_or_build_task(
        &self,
        conversation_id: &str,
        _options: BuildTaskOptions,
    ) -> Result<AgentInstance, AgentError> {
        let mut agents = self.agents.lock().unwrap();
        if let Some(agent) = agents.get(conversation_id) {
            return Ok(agent.clone());
        }

        let agent = AgentInstance::Mock(Arc::new(ScriptedAgent::new(conversation_id, Arc::clone(&self.sent))));
        agents.insert(conversation_id.to_owned(), agent.clone());
        Ok(agent)
    }

    fn kill(&self, conversation_id: &str, _reason: Option<AgentKillReason>) -> Result<(), AgentError> {
        self.agents.lock().unwrap().remove(conversation_id);
        Ok(())
    }

    fn kill_and_wait(
        &self,
        conversation_id: &str,
        reason: Option<AgentKillReason>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let _ = self.kill(conversation_id, reason);
        Box::pin(std::future::ready(()))
    }

    async fn clear(&self) {
        self.agents.lock().unwrap().clear();
    }

    fn active_count(&self) -> usize {
        self.agents.lock().unwrap().len()
    }

    fn collect_idle(&self, _idle_threshold_ms: TimestampMs) -> Vec<String> {
        Vec::new()
    }
}

fn bare_assistant_definition_params<'a>(
    definition_id: &'a str,
    assistant_id: &'a str,
    agent_id: &'a str,
) -> UpsertAssistantDefinitionParams<'a> {
    UpsertAssistantDefinitionParams {
        id: definition_id,
        assistant_id,
        source: "generated",
        owner_type: "system",
        source_ref: Some(assistant_id),
        name: assistant_id,
        name_i18n: "{}",
        description: Some("Channel bare assistant"),
        description_i18n: "{}",
        avatar_type: "emoji",
        avatar_value: Some("🤖"),
        agent_id,
        rule_resource_type: "user_file",
        rule_resource_ref: None,
        recommended_prompts: "[]",
        recommended_prompts_i18n: "{}",
        default_model_mode: "auto",
        default_model_value: None,
        default_permission_mode: "auto",
        default_permission_value: None,
        default_thought_level_mode: "auto",
        default_thought_level_value: None,
        default_skills_mode: "auto",
        default_skill_ids: "[]",
        custom_skill_names: "[]",
        default_disabled_builtin_skill_ids: "[]",
        default_mcps_mode: "auto",
        default_mcp_ids: "[]",
    }
}

#[tokio::test]
async fn send_to_agent_warms_cold_task_before_returning_stream_subscription() {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();
    SqliteClientPreferenceRepository::new(pool.clone())
        .upsert_batch(
            TEST_OWNER_USER_ID,
            &[
                (
                    "assistant.telegram.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.lark.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.dingtalk.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.weixin.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
            ],
        )
        .await
        .unwrap();

    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(RecordingTaskManager::new());
    let conversation_svc = Arc::new(ConversationService::new(
        std::env::temp_dir(),
        Arc::new(TestBroadcaster::new()),
        Arc::new(NoopSkillResolver),
        Arc::clone(&task_manager),
        Arc::new(SqliteConversationRepository::new(pool.clone())),
        Arc::new(SqliteAgentMetadataRepository::new(pool.clone())),
        Arc::new(SqliteAcpSessionRepository::new(pool.clone())),
    ));

    let settings = Arc::new(ChannelSettingsService::new(Arc::new(
        SqliteClientPreferenceRepository::new(pool),
    )));
    let message_svc = ChannelMessageService::new(conversation_svc, Arc::clone(&task_manager), settings);

    let session = AssistantSessionRow {
        id: "session-1".to_owned(),
        user_id: "channel-user-1".to_owned(),
        agent_type: "aionrs".to_owned(),
        conversation_id: None,
        workspace: None,
        chat_id: Some("7088048016".to_owned()),
        created_at: 1,
        last_activity: 1,
    };

    for platform in [
        PluginType::Telegram,
        PluginType::Lark,
        PluginType::Dingtalk,
        PluginType::Weixin,
    ] {
        let result = message_svc
            .send_to_agent(TEST_OWNER_USER_ID, &session, "hello", platform)
            .await
            .unwrap();

        assert!(
            result.stream_rx.is_some(),
            "channel relay must have an agent stream receiver after cold start for {platform:?}"
        );
        assert!(task_manager.get_task(&result.conversation_id).is_some());
    }
}

#[tokio::test]
async fn card_actions_reach_agent_with_server_history_and_reject_other_owner() {
    use dream_core_channel::action::ChatRequest;
    use std::time::Duration;
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();
    SqliteClientPreferenceRepository::new(pool.clone())
        .upsert_batch(
            TEST_OWNER_USER_ID,
            &[
                (
                    "assistant.telegram.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.lark.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.dingtalk.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.weixin.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
            ],
        )
        .await
        .unwrap();

    let manager = Arc::new(RecordingTaskManager::new());
    let task_manager: Arc<dyn IWorkerTaskManager> = manager.clone();
    let broadcaster = Arc::new(TestBroadcaster::new());
    let conversation_svc = Arc::new(ConversationService::new(
        std::env::temp_dir(),
        broadcaster.clone(),
        Arc::new(NoopSkillResolver),
        Arc::clone(&task_manager),
        Arc::new(SqliteConversationRepository::new(pool.clone())),
        Arc::new(SqliteAgentMetadataRepository::new(pool.clone())),
        Arc::new(SqliteAcpSessionRepository::new(pool.clone())),
    ));
    let settings = Arc::new(ChannelSettingsService::new(Arc::new(
        SqliteClientPreferenceRepository::new(pool),
    )));
    let svc = ChannelMessageService::new(conversation_svc, task_manager, settings);
    let mut session = AssistantSessionRow {
        id: "card-session".into(),
        user_id: "paired-user".into(),
        agent_type: "dream".into(),
        conversation_id: None,
        workspace: None,
        chat_id: Some("card-chat".into()),
        created_at: 1,
        last_activity: 1,
    };
    let missing = svc
        .send_chat_request(TEST_OWNER_USER_ID, &session, ChatRequest::Continue, PluginType::Lark)
        .await
        .unwrap_err();
    assert!(matches!(missing, ChannelError::InvalidConfig(_)));
    assert!(manager.sent.lock().unwrap().is_empty());
    let result = svc
        .send_to_agent(TEST_OWNER_USER_ID, &session, "original user prompt", PluginType::Lark)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "agent did not receive request: {error:?}; fixture events: {:?}",
                broadcaster.events.lock().unwrap()
            )
        });
    session.conversation_id = Some(result.conversation_id);
    for (expected_count, request) in [(1, ChatRequest::Continue), (2, ChatRequest::Regenerate)] {
        tokio::time::timeout(Duration::from_secs(3), async {
            while manager.sent.lock().unwrap().len() != expected_count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "agent did not receive request: {error:?}; fixture events: {:?}",
                broadcaster.events.lock().unwrap()
            )
        });
        tokio::task::yield_now().await;
        svc.send_chat_request(TEST_OWNER_USER_ID, &session, request, PluginType::Lark)
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while manager.sent.lock().unwrap().len() != 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        *manager.sent.lock().unwrap(),
        vec![
            "original user prompt",
            "Please continue your previous response.",
            "original user prompt"
        ]
    );
    let cross_owner = svc
        .send_chat_request("another-owner", &session, ChatRequest::Regenerate, PluginType::Lark)
        .await
        .unwrap_err();
    assert!(cross_owner.to_string().contains("not found"));
    assert_eq!(manager.sent.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn send_to_agent_persists_assistant_snapshot_for_channel_bound_assistant() {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();

    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(RecordingTaskManager::new());
    let conversation_repo = Arc::new(SqliteConversationRepository::new(pool.clone()));
    let conversation_repo_trait: Arc<dyn IConversationRepository> = conversation_repo.clone();
    let acp_session_repo = Arc::new(SqliteAcpSessionRepository::new(pool.clone()));
    let conversation_svc = Arc::new(ConversationService::new(
        std::env::temp_dir(),
        Arc::new(TestBroadcaster::new()),
        Arc::new(NoopSkillResolver),
        Arc::clone(&task_manager),
        conversation_repo_trait,
        Arc::new(SqliteAgentMetadataRepository::new(pool.clone())),
        acp_session_repo.clone(),
    ));

    let pref_repo = Arc::new(SqliteClientPreferenceRepository::new(pool.clone()));
    let definition_repo = Arc::new(SqliteAssistantDefinitionRepository::new(pool.clone()));
    let overlay_repo = Arc::new(SqliteAssistantOverlayRepository::new(pool.clone()));
    let assistant_preference_repo = Arc::new(SqliteAssistantPreferenceRepository::new(pool.clone()));
    conversation_svc.with_assistant_definition_repo(definition_repo.clone());
    conversation_svc.with_assistant_state_repo(overlay_repo.clone());
    conversation_svc.with_assistant_preference_repo(assistant_preference_repo);
    definition_repo
        .upsert(&bare_assistant_definition_params(
            "asstdef-channel-claude",
            "bare-claude",
            "claude",
        ))
        .await
        .unwrap();
    pref_repo
        .upsert_batch(
            TEST_OWNER_USER_ID,
            &[(
                "assistant.telegram.agent",
                r#"{"assistant_id":"bare-claude","name":"Claude"}"#,
            )],
        )
        .await
        .unwrap();

    let settings = Arc::new(ChannelSettingsService::new(pref_repo).with_assistant_repos(definition_repo, overlay_repo));
    let message_svc = ChannelMessageService::new(conversation_svc, Arc::clone(&task_manager), settings);

    let session = AssistantSessionRow {
        id: "session-assisted".to_owned(),
        user_id: "channel-user-1".to_owned(),
        agent_type: "aionrs".to_owned(),
        conversation_id: None,
        workspace: None,
        chat_id: Some("7088048016".to_owned()),
        created_at: 1,
        last_activity: 1,
    };

    let result = message_svc
        .send_to_agent(TEST_OWNER_USER_ID, &session, "hello", PluginType::Telegram)
        .await
        .unwrap();

    let snapshot = conversation_repo
        .get_assistant_snapshot(TEST_OWNER_USER_ID, &result.conversation_id)
        .await
        .unwrap();
    assert!(
        snapshot.is_some(),
        "channel-created conversation should persist an assistant snapshot when the platform is bound to an assistant"
    );
    let snapshot = snapshot.unwrap();
    let conversation = conversation_repo
        .get(TEST_OWNER_USER_ID, &result.conversation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(conversation.r#type, AgentType::Acp.serde_name());
    let session_row = acp_session_repo
        .get_for_user(TEST_OWNER_USER_ID, &result.conversation_id)
        .await
        .unwrap()
        .expect("acp_session row should exist for ACP assistant conversations");
    assert_eq!(session_row.agent_id, "2d23ff1c");
    assert_eq!(snapshot.assistant_id, "bare-claude");
    assert_eq!(snapshot.agent_id, "2d23ff1c");
    assert_eq!(conversation.name, "Claude");
}

#[tokio::test]
async fn send_to_agent_rejects_unresolvable_channel_assistant_binding() {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();

    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(RecordingTaskManager::new());
    let conversation_repo = Arc::new(SqliteConversationRepository::new(pool.clone()));
    let conversation_repo_trait: Arc<dyn IConversationRepository> = conversation_repo.clone();
    let acp_session_repo = Arc::new(SqliteAcpSessionRepository::new(pool.clone()));
    let conversation_svc = Arc::new(ConversationService::new(
        std::env::temp_dir(),
        Arc::new(TestBroadcaster::new()),
        Arc::new(NoopSkillResolver),
        Arc::clone(&task_manager),
        conversation_repo_trait,
        Arc::new(SqliteAgentMetadataRepository::new(pool.clone())),
        acp_session_repo,
    ));

    let pref_repo = Arc::new(SqliteClientPreferenceRepository::new(pool.clone()));
    pref_repo
        .upsert_batch(
            TEST_OWNER_USER_ID,
            &[(
                "assistant.telegram.agent",
                r#"{"assistant_id":"missing-assistant","name":"Missing"}"#,
            )],
        )
        .await
        .unwrap();
    let definition_repo = Arc::new(SqliteAssistantDefinitionRepository::new(pool.clone()));
    let overlay_repo = Arc::new(SqliteAssistantOverlayRepository::new(pool.clone()));
    let settings = Arc::new(ChannelSettingsService::new(pref_repo).with_assistant_repos(definition_repo, overlay_repo));
    let message_svc = ChannelMessageService::new(conversation_svc, Arc::clone(&task_manager), settings);

    let session = AssistantSessionRow {
        id: "session-assisted-missing".to_owned(),
        user_id: "channel-user-missing".to_owned(),
        agent_type: "aionrs".to_owned(),
        conversation_id: None,
        workspace: None,
        chat_id: Some("7088048017".to_owned()),
        created_at: 1,
        last_activity: 1,
    };

    let err = message_svc
        .send_to_agent(TEST_OWNER_USER_ID, &session, "hello", PluginType::Telegram)
        .await
        .unwrap_err();
    assert!(matches!(err, ChannelError::MessageSendFailed(_)));
    assert!(
        err.to_string().contains("missing-assistant"),
        "error should surface the unresolved assistant identity"
    );
}

#[tokio::test]
async fn send_to_agent_without_saved_binding_defaults_to_bare_dream_engine_assistant() {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();
    SqliteClientPreferenceRepository::new(pool.clone())
        .upsert_batch(
            TEST_OWNER_USER_ID,
            &[
                (
                    "assistant.telegram.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.lark.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.dingtalk.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.weixin.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
            ],
        )
        .await
        .unwrap();

    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(RecordingTaskManager::new());
    let conversation_repo = Arc::new(SqliteConversationRepository::new(pool.clone()));
    let conversation_repo_trait: Arc<dyn IConversationRepository> = conversation_repo.clone();
    let acp_session_repo = Arc::new(SqliteAcpSessionRepository::new(pool.clone()));
    let conversation_svc = Arc::new(ConversationService::new(
        std::env::temp_dir(),
        Arc::new(TestBroadcaster::new()),
        Arc::new(NoopSkillResolver),
        Arc::clone(&task_manager),
        conversation_repo_trait,
        Arc::new(SqliteAgentMetadataRepository::new(pool.clone())),
        acp_session_repo,
    ));

    let pref_repo = Arc::new(SqliteClientPreferenceRepository::new(pool.clone()));
    let definition_repo = Arc::new(SqliteAssistantDefinitionRepository::new(pool.clone()));
    let overlay_repo = Arc::new(SqliteAssistantOverlayRepository::new(pool.clone()));
    let assistant_preference_repo = Arc::new(SqliteAssistantPreferenceRepository::new(pool.clone()));
    conversation_svc.with_assistant_definition_repo(definition_repo.clone());
    conversation_svc.with_assistant_state_repo(overlay_repo.clone());
    conversation_svc.with_assistant_preference_repo(assistant_preference_repo);
    definition_repo
        .upsert(&bare_assistant_definition_params(
            "asstdef-channel-dream-engine",
            "bare-dream-engine",
            "dream",
        ))
        .await
        .unwrap();

    let settings = Arc::new(ChannelSettingsService::new(pref_repo).with_assistant_repos(definition_repo, overlay_repo));
    let message_svc = ChannelMessageService::new(conversation_svc, Arc::clone(&task_manager), settings);

    let session = AssistantSessionRow {
        id: "session-assisted-default-dream-engine".to_owned(),
        user_id: "channel-user-default".to_owned(),
        agent_type: "dream".to_owned(),
        conversation_id: None,
        workspace: None,
        chat_id: Some("7088048018".to_owned()),
        created_at: 1,
        last_activity: 1,
    };

    let result = message_svc
        .send_to_agent(TEST_OWNER_USER_ID, &session, "hello", PluginType::Telegram)
        .await
        .unwrap();

    let snapshot = conversation_repo
        .get_assistant_snapshot(TEST_OWNER_USER_ID, &result.conversation_id)
        .await
        .unwrap()
        .expect("channel-created conversation should default to a bare assistant snapshot");
    let conversation = conversation_repo
        .get(TEST_OWNER_USER_ID, &result.conversation_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(snapshot.assistant_id, "bare-dream-engine");
    assert_eq!(snapshot.agent_id, "632f31d2");
    assert_eq!(conversation.r#type, AgentType::DreamEngine.serde_name());
    assert_eq!(conversation.name, "tg-dream-70880480");
}

#[tokio::test]
async fn send_to_agent_without_assistant_name_falls_back_to_legacy_channel_name() {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();

    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(RecordingTaskManager::new());
    let conversation_repo = Arc::new(SqliteConversationRepository::new(pool.clone()));
    let conversation_repo_trait: Arc<dyn IConversationRepository> = conversation_repo.clone();
    let acp_session_repo = Arc::new(SqliteAcpSessionRepository::new(pool.clone()));
    let conversation_svc = Arc::new(ConversationService::new(
        std::env::temp_dir(),
        Arc::new(TestBroadcaster::new()),
        Arc::new(NoopSkillResolver),
        Arc::clone(&task_manager),
        conversation_repo_trait,
        Arc::new(SqliteAgentMetadataRepository::new(pool.clone())),
        acp_session_repo,
    ));

    let pref_repo = Arc::new(SqliteClientPreferenceRepository::new(pool.clone()));
    let definition_repo = Arc::new(SqliteAssistantDefinitionRepository::new(pool.clone()));
    let overlay_repo = Arc::new(SqliteAssistantOverlayRepository::new(pool.clone()));
    let assistant_preference_repo = Arc::new(SqliteAssistantPreferenceRepository::new(pool.clone()));
    conversation_svc.with_assistant_definition_repo(definition_repo.clone());
    conversation_svc.with_assistant_state_repo(overlay_repo.clone());
    conversation_svc.with_assistant_preference_repo(assistant_preference_repo);
    definition_repo
        .upsert(&bare_assistant_definition_params(
            "asstdef-channel-codex",
            "bare-codex",
            "codex",
        ))
        .await
        .unwrap();
    pref_repo
        .upsert_batch(
            TEST_OWNER_USER_ID,
            &[("assistant.telegram.agent", r#"{"assistant_id":"bare-codex"}"#)],
        )
        .await
        .unwrap();

    let settings = Arc::new(ChannelSettingsService::new(pref_repo).with_assistant_repos(definition_repo, overlay_repo));
    let message_svc = ChannelMessageService::new(conversation_svc, Arc::clone(&task_manager), settings);

    let session = AssistantSessionRow {
        id: "session-assisted-fallback-name".to_owned(),
        user_id: "channel-user-2".to_owned(),
        agent_type: "aionrs".to_owned(),
        conversation_id: None,
        workspace: None,
        chat_id: Some("7088048016".to_owned()),
        created_at: 1,
        last_activity: 1,
    };

    let result = message_svc
        .send_to_agent(TEST_OWNER_USER_ID, &session, "hello", PluginType::Telegram)
        .await
        .unwrap();

    let conversation = conversation_repo
        .get(TEST_OWNER_USER_ID, &result.conversation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(conversation.name, "tg-acp-codex-70880480");
}

/// Always refuses — T3: proves an IM-channel-triggered send is now stopped
/// by the send gate instead of bypassing it entirely (the gate used to only
/// be checked by the HTTP `send_msg` handler; `send_to_agent` calls
/// `ConversationService::send_message` directly and never went through it).
/// Scope note: `send_to_agent` still runs conversation creation/warmup
/// BEFORE calling `send_message`, so those steps still happen even when the
/// gate ultimately refuses — the gate stops the metered turn itself
/// (message persistence + agent dispatch), not everything upstream of it.
struct RejectingSendGate;

#[async_trait]
impl dream_core_conversation::SendGate for RejectingSendGate {
    async fn check_send(
        &self,
        _user_id: &str,
        _model: Option<&str>,
    ) -> Result<(), dream_core_conversation::PolicyDenial> {
        Err(dream_core_conversation::PolicyDenial::new(
            "BUDGET_EXCEEDED",
            "the team's monthly spend budget is exhausted",
        ))
    }

    async fn check_model(&self, _user_id: &str, _model: &str) -> Result<(), dream_core_conversation::PolicyDenial> {
        Ok(())
    }
}

#[tokio::test]
async fn send_to_agent_is_blocked_by_a_rejecting_send_gate() {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();
    SqliteClientPreferenceRepository::new(pool.clone())
        .upsert_batch(
            TEST_OWNER_USER_ID,
            &[
                (
                    "assistant.telegram.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.lark.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.dingtalk.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
                (
                    "assistant.weixin.defaultModel",
                    r#"{"id":"test-provider","use_model":"test-model"}"#,
                ),
            ],
        )
        .await
        .unwrap();

    let task_manager: Arc<dyn IWorkerTaskManager> = Arc::new(RecordingTaskManager::new());
    let conversation_svc = Arc::new(ConversationService::new(
        std::env::temp_dir(),
        Arc::new(TestBroadcaster::new()),
        Arc::new(NoopSkillResolver),
        Arc::clone(&task_manager),
        Arc::new(SqliteConversationRepository::new(pool.clone())),
        Arc::new(SqliteAgentMetadataRepository::new(pool.clone())),
        Arc::new(SqliteAcpSessionRepository::new(pool.clone())),
    ));
    conversation_svc.with_send_gate(Arc::new(RejectingSendGate));

    let settings = Arc::new(ChannelSettingsService::new(Arc::new(
        SqliteClientPreferenceRepository::new(pool),
    )));
    let message_svc = ChannelMessageService::new(conversation_svc, Arc::clone(&task_manager), settings);

    let session = AssistantSessionRow {
        id: "session-gated".to_owned(),
        user_id: "channel-user-gated".to_owned(),
        agent_type: "aionrs".to_owned(),
        conversation_id: None,
        workspace: None,
        chat_id: Some("7088048016".to_owned()),
        created_at: 1,
        last_activity: 1,
    };

    let err = message_svc
        .send_to_agent(TEST_OWNER_USER_ID, &session, "hello", PluginType::Telegram)
        .await
        .unwrap_err();

    match err {
        ChannelError::MessageSendFailed(message) => {
            assert!(
                message.contains("budget"),
                "expected the denial's message, got: {message}"
            );
        }
        other => panic!("expected MessageSendFailed carrying the policy denial, got {other:?}"),
    }
}
