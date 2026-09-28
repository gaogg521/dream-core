use dream_core_db::init_database_memory;
use sqlx::Executor;
use sqlx::Row;

const MIGRATION_SQL: &str = include_str!("../migrations/059_replace_1one_cli_mascot_with_brand_logo.sql");
const MASCOT: &str = "/api/assets/logos/brand/1one.png";
const BRAND_LOGO: &str = "/api/assets/logos/brand/1one-cli.svg";

async fn run_migration(pool: &sqlx::SqlitePool) {
    pool.execute(sqlx::raw_sql(MIGRATION_SQL)).await.unwrap();
}

#[tokio::test]
async fn internal_1one_cli_and_its_generated_assistant_use_the_product_mark() {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool();

    sqlx::query("UPDATE agent_metadata SET icon = ? WHERE agent_id = '632f31d2'")
        .bind(MASCOT)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO assistant_definitions (
            id, assistant_id, source, owner_type, source_ref, name, name_i18n,
            description_i18n, avatar_type, avatar_value, agent_id, rule_resource_type,
            recommended_prompts, recommended_prompts_i18n, default_model_mode,
            default_permission_mode, default_skills_mode, default_skill_ids,
            custom_skill_names, default_disabled_builtin_skill_ids, default_mcps_mode,
            default_mcp_ids, created_at, updated_at
         ) VALUES (
            'one-cli-generated', 'bare:632f31d2', 'generated', 'system', '632f31d2', '1ONE CLI', '{}',
            '{}', 'emoji', ?, '632f31d2', 'none', '[]', '{}', 'auto', 'auto', 'auto', '[]', '[]', '[]',
            'auto', '[]', 1, 1
         )",
    )
    .bind(MASCOT)
    .execute(pool)
    .await
    .unwrap();

    run_migration(pool).await;

    let icon: String = sqlx::query("SELECT icon FROM agent_metadata WHERE agent_id = '632f31d2'")
        .fetch_one(pool)
        .await
        .unwrap()
        .get("icon");
    assert_eq!(icon, BRAND_LOGO);

    let avatar =
        sqlx::query("SELECT avatar_type, avatar_value FROM assistant_definitions WHERE id = 'one-cli-generated'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(avatar.get::<String, _>("avatar_type"), "builtin_asset");
    assert_eq!(avatar.get::<String, _>("avatar_value"), BRAND_LOGO);
}

#[tokio::test]
async fn rerunning_the_migration_leaves_the_new_logo_untouched() {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool();

    run_migration(pool).await;
    let first_updated_at: i64 = sqlx::query("SELECT updated_at FROM agent_metadata WHERE agent_id = '632f31d2'")
        .fetch_one(pool)
        .await
        .unwrap()
        .get("updated_at");
    run_migration(pool).await;
    let second_updated_at: i64 = sqlx::query("SELECT updated_at FROM agent_metadata WHERE agent_id = '632f31d2'")
        .fetch_one(pool)
        .await
        .unwrap()
        .get("updated_at");

    assert_eq!(first_updated_at, second_updated_at);
}
