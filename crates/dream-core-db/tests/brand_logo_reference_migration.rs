//! Migration 057 repoints anything still referencing the pre-rebrand brand logo.
//!
//! The asset it points at (`logos/brand/aion.svg`) was deleted from the embedded
//! `dream-core-assets` corpus, so a row this migration misses does not error — it
//! renders a broken image where an agent or assistant avatar should be, forever,
//! with nothing in the logs.
//!
//! Migrations 001 and 021 already moved the rows they knew about, but both carry a
//! narrow `WHERE` (001 seeded only `agent_type = 'aionrs' AND agent_source =
//! 'internal'`; 021 additionally required `source = 'generated'` on the assistant
//! side). A user-edited agent, or an assistant created from another source, kept
//! the old path. This migration deliberately matches on the PATH instead, so the
//! test seeds exactly the rows those predicates would have skipped.
//!
//! As in `builtin_assistant_rebrand_migration`, the SQL file is executed directly:
//! `init_database_memory()` has already run every migration, so a `Migrator` pass
//! would skip 057 and every assertion below would hold vacuously.

use dream_core_db::init_database_memory;
use sqlx::Executor;
use sqlx::Row;

const MIGRATION_SQL: &str = include_str!("../migrations/057_drop_legacy_brand_logo_reference.sql");
const LEGACY_LOGO: &str = "/api/assets/logos/brand/aion.svg";
const CURRENT_LOGO: &str = "/api/assets/logos/brand/1one.png";

async fn run_migration(pool: &sqlx::SqlitePool) {
    pool.execute(sqlx::raw_sql(MIGRATION_SQL)).await.unwrap();
}

/// A fully-migrated database re-seeded with rows the earlier migrations' `WHERE`
/// clauses would not have matched.
async fn pool_with_stale_logo_rows() -> sqlx::SqlitePool {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();
    // Leak the Database so the in-memory pool outlives this helper.
    std::mem::forget(db);

    // A user-edited agent: not `agent_source = 'internal'`, so migration 021 skipped it.
    sqlx::query(
        "INSERT INTO agent_metadata (
            id, agent_id, name, backend, command, agent_type, enabled, agent_source,
            sort_order, icon, created_at, updated_at
         ) VALUES (
            'agent-user-edited', 'agent-user-edited', 'My CLI', NULL, '', 'dream', 1, 'user',
            100, ?, 1, 1
         )",
    )
    .bind(LEGACY_LOGO)
    .execute(&pool)
    .await
    .unwrap();

    // An agent already on the current asset — it must come out unchanged, and its
    // `updated_at` must not be bumped by a migration that had nothing to do.
    sqlx::query(
        "INSERT INTO agent_metadata (
            id, agent_id, name, backend, command, agent_type, enabled, agent_source,
            sort_order, icon, created_at, updated_at
         ) VALUES (
            'agent-already-current', 'agent-already-current', 'Current CLI', NULL, '', 'dream', 1, 'user',
            101, ?, 1, 1
         )",
    )
    .bind(CURRENT_LOGO)
    .execute(&pool)
    .await
    .unwrap();

    // An assistant whose `source` is not 'generated', so migration 021 skipped it.
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
            'def-user-made', NULL, 'user-made', 'user', 'user', 'user-made',
            'Mine', '{}', NULL, '{}', 'builtin_asset', ?,
            'dream', 'none', NULL,
            '[]', '{}',
            'auto', 'auto',
            'auto', '[]', '[]',
            '[]',
            'auto', '[]', 1, 1
         )",
    )
    .bind(LEGACY_LOGO)
    .execute(&pool)
    .await
    .unwrap();

    pool
}

#[tokio::test]
async fn a_user_edited_agent_is_moved_off_the_deleted_asset() {
    let pool = pool_with_stale_logo_rows().await;
    run_migration(&pool).await;

    let icon: String = sqlx::query("SELECT icon FROM agent_metadata WHERE id = 'agent-user-edited'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("icon");

    assert_eq!(
        icon, CURRENT_LOGO,
        "the row migrations 001/021 could not reach must be repointed"
    );
}

/// The assistant side has its own `WHERE`, and its own column — `avatar_type` has
/// to be corrected alongside `avatar_value` or the frontend reads the path with the
/// wrong loader.
#[tokio::test]
async fn a_user_made_assistant_is_moved_off_the_deleted_asset() {
    let pool = pool_with_stale_logo_rows().await;
    run_migration(&pool).await;

    let row = sqlx::query("SELECT avatar_type, avatar_value FROM assistant_definitions WHERE id = 'def-user-made'")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(row.get::<String, _>("avatar_value"), CURRENT_LOGO);
    assert_eq!(row.get::<String, _>("avatar_type"), "builtin_asset");
}

/// Matching on the path means the migration is idempotent by construction. This is
/// what makes it safe to execute directly here, and safe to re-run on a database
/// that has already had it applied.
#[tokio::test]
async fn a_second_pass_changes_nothing() {
    let pool = pool_with_stale_logo_rows().await;
    run_migration(&pool).await;

    let after_first: i64 = sqlx::query("SELECT updated_at FROM agent_metadata WHERE id = 'agent-user-edited'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("updated_at");

    run_migration(&pool).await;

    let after_second: i64 = sqlx::query("SELECT updated_at FROM agent_metadata WHERE id = 'agent-user-edited'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("updated_at");

    assert_eq!(
        after_first, after_second,
        "a second pass must not touch an already-migrated row"
    );
}

/// A row that was already on the current asset must not be rewritten — if the
/// `WHERE` were dropped, every avatar in the database would be flattened to the
/// brand logo and the damage would be invisible until someone looked.
#[tokio::test]
async fn rows_already_on_the_current_asset_are_left_alone() {
    let pool = pool_with_stale_logo_rows().await;

    let before: i64 = sqlx::query("SELECT updated_at FROM agent_metadata WHERE id = 'agent-already-current'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("updated_at");

    run_migration(&pool).await;

    let row = sqlx::query("SELECT icon, updated_at FROM agent_metadata WHERE id = 'agent-already-current'")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(row.get::<String, _>("icon"), CURRENT_LOGO);
    assert_eq!(
        row.get::<i64, _>("updated_at"),
        before,
        "an untouched row must keep its timestamp"
    );
}

/// The asset the migration moves rows OFF must be gone from the embedded corpus.
/// If it were still shipping, this migration would be pointless churn; if a row
/// still referenced it after this migration, the reference would be dangling.
#[tokio::test]
async fn no_row_references_the_deleted_asset_afterwards() {
    let pool = pool_with_stale_logo_rows().await;
    run_migration(&pool).await;

    let agents: i64 = sqlx::query("SELECT COUNT(*) AS n FROM agent_metadata WHERE icon = ?")
        .bind(LEGACY_LOGO)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    let assistants: i64 = sqlx::query("SELECT COUNT(*) AS n FROM assistant_definitions WHERE avatar_value = ?")
        .bind(LEGACY_LOGO)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");

    assert_eq!(agents, 0);
    assert_eq!(assistants, 0);
}
