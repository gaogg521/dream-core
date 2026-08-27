//! Migration 053 moves the built-in butler and its four skills off the
//! upstream brand.
//!
//! The failure mode this guards is not a crash. `source_ref` is the manifest's
//! identity column (unique with `source`), and the manifest is re-seeded on
//! every boot under the NEW ids — so a row this migration fails to rename is
//! seeded a second time instead of matching, and the user ends up with two
//! butlers. A row it renames incorrectly is worse: the butler and its skills
//! simply stop resolving.
//!
//! The database is built with `init_database_memory()` (every migration
//! applied), then seeded with a legacy-shaped row and put through migration 053
//! again. That is sound precisely because the migration must be idempotent —
//! `a_second_pass_changes_nothing` is the test that holds that property up.

use dream_core_db::init_database_memory;
use sqlx::Executor;
use sqlx::Row;

const REBRAND_SQL: &str = include_str!("../migrations/053_rebrand_builtin_assistant_and_skills.sql");

/// Apply the migration's SQL directly.
///
/// `init_database_memory()` has already run every migration, so sqlx's version
/// ledger would make `Migrator` skip this one — the seeded legacy row would
/// sail through untouched and every assertion below would pass vacuously.
/// Executing the file is also what puts the real statements under test rather
/// than a paraphrase of them.
async fn run_rebrand(pool: &sqlx::SqlitePool) {
    pool.execute(sqlx::raw_sql(REBRAND_SQL)).await.unwrap();
}

/// A fully-migrated database re-seeded with a butler under the old identity.
async fn pool_with_legacy_butler() -> sqlx::SqlitePool {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();
    // Leak the Database so the in-memory pool outlives this helper.
    std::mem::forget(db);

    sqlx::query(
        "INSERT INTO assistant_definitions (
            id, user_id, assistant_id, source, owner_type, source_ref,
            name, name_i18n, description, description_i18n, avatar_type, avatar_value,
            agent_id, rule_resource_type, rule_resource_ref,
            recommended_prompts, recommended_prompts_i18n,
            default_model_mode, default_permission_mode,
            default_skills_mode, default_skill_ids, custom_skill_names,
            default_disabled_builtin_skill_ids,
            default_mcps_mode, default_mcp_ids, created_at, updated_at
         ) VALUES (
            'def-butler', NULL, 'aionui-assistant', 'builtin', 'system', 'aionui-assistant',
            'Butler', '{}', NULL, '{}', 'builtin_asset', 'avatars/aionui-assistant.jpg',
            'dream', 'builtin_asset', 'aionui-assistant',
            '[]', '{}',
            'auto', 'auto',
            'auto', '[\"aionui-config\",\"aionui-troubleshooting\",\"aionui-webui-public\"]', '[]',
            '[\"aionui-webui-setup\"]',
            'auto', '[]', 1, 1
         )",
    )
    .execute(&pool)
    .await
    .unwrap();

    pool
}

#[tokio::test]
async fn butler_identity_moves_off_the_upstream_brand() {
    let pool = pool_with_legacy_butler().await;
    run_rebrand(&pool).await;

    let row = sqlx::query(
        "SELECT assistant_id, source_ref, rule_resource_ref, avatar_value
         FROM assistant_definitions WHERE id = 'def-butler'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.get::<String, _>("assistant_id"), "one-assistant");
    assert_eq!(row.get::<String, _>("source_ref"), "one-assistant");
    assert_eq!(row.get::<String, _>("rule_resource_ref"), "one-assistant");
    assert_eq!(row.get::<String, _>("avatar_value"), "avatars/one-assistant.jpg");
}

/// The skill lists are JSON arrays in TEXT columns. Getting these wrong does not
/// error — the butler simply comes up without the skills that make it useful.
#[tokio::test]
async fn the_butlers_skill_lists_are_rewritten() {
    let pool = pool_with_legacy_butler().await;
    run_rebrand(&pool).await;

    let row = sqlx::query(
        "SELECT default_skill_ids, default_disabled_builtin_skill_ids
         FROM assistant_definitions WHERE id = 'def-butler'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let enabled: Vec<String> = serde_json::from_str(&row.get::<String, _>("default_skill_ids")).unwrap();
    assert_eq!(enabled, ["one-config", "one-troubleshooting", "one-webui-public"]);

    let disabled: Vec<String> =
        serde_json::from_str(&row.get::<String, _>("default_disabled_builtin_skill_ids")).unwrap();
    assert_eq!(disabled, ["one-webui-setup"]);
}

/// Anything not one of the five renamed ids must survive untouched — the
/// replacement targets quoted JSON tokens precisely so a longer id that merely
/// starts the same way is not mangled.
#[tokio::test]
async fn unrelated_skill_ids_are_left_alone() {
    let pool = pool_with_legacy_butler().await;
    sqlx::query(
        "UPDATE assistant_definitions
         SET default_skill_ids = '[\"aionui-config\",\"aionui-config-extra\",\"canvas-design\"]'
         WHERE id = 'def-butler'",
    )
    .execute(&pool)
    .await
    .unwrap();

    run_rebrand(&pool).await;

    let raw: String = sqlx::query("SELECT default_skill_ids FROM assistant_definitions WHERE id = 'def-butler'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("default_skill_ids");
    let ids: Vec<String> = serde_json::from_str(&raw).unwrap();

    assert_eq!(ids, ["one-config", "aionui-config-extra", "canvas-design"]);
}

/// A scheduled task binds its assistant inside a JSON blob. Left behind, it
/// resolves to nothing and fails at its next run — long after the upgrade, with
/// nothing to connect the two.
#[tokio::test]
async fn scheduled_tasks_follow_the_butler() {
    let pool = pool_with_legacy_butler().await;
    sqlx::query(
        "INSERT INTO cron_jobs (
            id, user_id, name, enabled, schedule_kind, schedule_value, execution_mode, payload_message, conversation_id, created_by,
            agent_config, created_at, updated_at
         ) VALUES (
            'job-1', 'user_1', 'nightly', 1, 'cron', '0 0 * * *', 'existing', 'run it', 'conv-1', 'user',
            '{\"assistant_id\":\"aionui-assistant\",\"model_id\":\"gpt-5\"}', 1, 1
         )",
    )
    .execute(&pool)
    .await
    .unwrap();

    run_rebrand(&pool).await;

    let config: String = sqlx::query("SELECT agent_config FROM cron_jobs WHERE id = 'job-1'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("agent_config");
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();

    assert_eq!(parsed["assistant_id"], "one-assistant");
    // Untouched siblings must survive the json_set.
    assert_eq!(parsed["model_id"], "gpt-5");
}

/// Running it twice must not undo or duplicate anything — a migration that is
/// not idempotent turns a partially-applied upgrade into a corrupt one.
#[tokio::test]
async fn a_second_pass_changes_nothing() {
    let pool = pool_with_legacy_butler().await;
    run_rebrand(&pool).await;
    run_rebrand(&pool).await;

    let count: i64 =
        sqlx::query("SELECT COUNT(*) AS c FROM assistant_definitions WHERE assistant_id = 'one-assistant'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("c");
    assert_eq!(count, 1);
}
