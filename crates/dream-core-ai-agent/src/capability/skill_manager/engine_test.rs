use std::sync::Arc;

use dream_core_db::{ISkillRepository, SqliteSkillRepository, UpsertSkillParams, init_database_memory};
use dream_core_extension::{
    SkillPaths,
    team_sync::{TeamSkillPayload, sync_team_skills},
};
use dream_engine_agent::{bootstrap::AgentBootstrap, output::null_sink::NullSink};
use dream_engine_config::{
    compat::ProviderCompat,
    config::{Config, ProviderType},
};
use serde_json::json;

use super::AcpSkillManager;

#[tokio::test]
async fn employee_skill_from_500_catalog_loads_body_without_foreign_or_unselected_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let paths = SkillPaths {
        data_dir: root.to_path_buf(),
        user_skills_dir: root.join("skills"),
        cron_skills_dir: root.join("cron/skills"),
        builtin_skills_dir: root.join("builtin-skills"),
        builtin_rules_dir: root.join("rules"),
        assistant_rules_dir: root.join("assistant-rules"),
        assistant_skills_dir: root.join("assistant-skills"),
    };
    let payloads: Vec<_> = (1..=500)
        .map(|i| TeamSkillPayload {
            id: format!("oskill_{i}"),
            name: format!("bound-skill-{i}"),
            description: "QA".into(),
            content: format!("BOUND-SKILL-{i}-BODY"),
            auto_active: i == 1,
            category: None,
            tags: vec![],
        })
        .collect();
    sync_team_skills(&paths.team_skills_dir(), &payloads, true)
        .await
        .unwrap();
    let foreign = root.join("foreign");
    tokio::fs::create_dir_all(&foreign).await.unwrap();
    tokio::fs::write(foreign.join("SKILL.md"), "FOREIGN-SECRET-BODY")
        .await
        .unwrap();
    let db = init_database_memory().await.unwrap();
    for user in ["user-a", "user-b"] {
        sqlx::query("INSERT INTO users (id, user_type, external_user_id, username, status, session_generation, created_at, updated_at) VALUES (?, 'aionpro', ?, ?, 'active', 0, 1, 1)")
            .bind(user).bind(user).bind(user).execute(db.pool()).await.unwrap();
    }
    let repo = Arc::new(SqliteSkillRepository::new(db.pool().clone()));
    repo.upsert_for_user(
        "user-b",
        UpsertSkillParams {
            name: "foreign-skill",
            description: None,
            display_name: None,
            icon_file: None,
            path: foreign.to_str().unwrap(),
            source: "user",
            enabled: true,
        },
    )
    .await
    .unwrap();
    let manager = AcpSkillManager::new_with_repo(Arc::new(paths), repo);
    let requested = vec!["bound-skill-500".into(), "foreign-skill".into()];
    let skills = manager.engine_skills_for_user("user-a", &requested).await.unwrap();
    assert_eq!(
        skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["bound-skill-1", "bound-skill-500"]
    );
    let selected = skills.iter().find(|s| s.name == "bound-skill-500").unwrap();
    assert!(
        std::path::Path::new(selected.skill_root.as_ref().unwrap())
            .join("SKILL.md")
            .is_file()
    );
    let config = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "qa-no-network".into(),
        base_url: "http://localhost:0".into(),
        model: "qa-model".into(),
        max_tokens: None,
        max_turns: Some(1),
        max_tool_call_malformed_turns: None,
        max_tool_call_failure_turns: None,
        system_prompt: None,
        thinking: None,
        prompt_caching: false,
        compat: ProviderCompat::openai_defaults(),
        tools: Default::default(),
        session: Default::default(),
        compact: Default::default(),
        plan: Default::default(),
        shell: Default::default(),
        file_cache: Default::default(),
        hooks: Default::default(),
        bedrock: None,
        vertex: None,
        mcp: Default::default(),
        logging: Default::default(),
        vision: None,
        local_ocr: None,
        extra_headers: None,
    };
    let mut engine = AgentBootstrap::new(config, root.to_str().unwrap(), Arc::new(NullSink))
        .host_skills(skills)
        .build()
        .await
        .unwrap()
        .engine;
    let loaded = engine
        .registry_mut()
        .get("Skill")
        .unwrap()
        .execute(json!({"skill":"bound-skill-500"}))
        .await;
    assert!(!loaded.is_error, "{}", loaded.content);
    assert!(loaded.content.contains("BOUND-SKILL-500-BODY"));
    for name in ["bound-skill-499", "foreign-skill"] {
        let rejected = engine
            .registry_mut()
            .get("Skill")
            .unwrap()
            .execute(json!({"skill":name}))
            .await;
        assert!(rejected.is_error);
        assert!(!rejected.content.contains("FOREIGN-SECRET-BODY"));
    }
}
