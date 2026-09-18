use dream_core_db::{IAgentMetadataRepository, SqliteAgentMetadataRepository, init_database_memory};

/// Migration 038: the builtin dream agent (Dream CLI, seed id `632f31d2`)
/// carries a constructed at-turn fork capability — the same shape 036 wrote
/// for codex (turn anchors are stamped by the dream manager + engine).
#[tokio::test]
async fn dream_engine_builtin_agent_declares_at_turn_fork_capability() {
    let db = init_database_memory().await.unwrap();
    let repo = SqliteAgentMetadataRepository::new(db.pool().clone());

    let dream_engine = repo.get("632f31d2").await.unwrap().expect("seeded Aion CLI row");
    assert_eq!(dream_engine.agent_type, "dream");
    assert_eq!(
        dream_engine.backend, None,
        "dream-engine resolves by agent_type, not backend"
    );

    let capabilities: serde_json::Value = serde_json::from_str(
        dream_engine
            .agent_capabilities
            .as_deref()
            .expect("constructed capabilities"),
    )
    .unwrap();
    assert_eq!(
        capabilities["session_capabilities"]["fork"],
        serde_json::json!({"at_turn": true}),
        "at-turn fork: anchors are stamped on dream_engine rows and session messages"
    );
}
