//! Migration runner for one-employee tables.
//!
//! Shares the `_one_migrations` ledger with one-org; entry names carry the
//! `employee_` prefix so the two crates' key spaces stay disjoint. Same
//! append-only rules: never edit or reorder shipped entries. Since P3-3 the
//! runner logic lives in `dream_core_db::run_ledgered_migrations`; this file
//! only carries the two migration trees (SQLite `migrations/`, MySQL
//! `migrations_mysql/`) and hands them to the shared runner keyed by the
//! pool's backend.

use dream_core_db::{DbPool, MigrationSet, run_ledgered_migrations};

use crate::error::EmployeeError;

const MIGRATIONS: &[(&str, &str)] = &[
    ("employee_001_init", include_str!("../migrations/001_init.sql")),
    ("employee_002_schedule", include_str!("../migrations/002_schedule.sql")),
    (
        "employee_003_visibility",
        include_str!("../migrations/003_visibility.sql"),
    ),
    (
        "employee_004_persona_model",
        include_str!("../migrations/004_persona_and_model.sql"),
    ),
    // Was never registered here despite the file shipping — a real bug found
    // while adding 006 below, fixed alongside it since both touch this same
    // array. Safe to backfill now: the UPDATE it runs is idempotent (matches
    // zero rows once already applied).
    (
        "employee_005_dream_rebrand_agent_type",
        include_str!("../migrations/005_dream_rebrand_agent_type.sql"),
    ),
    ("employee_006_grants", include_str!("../migrations/006_grants.sql")),
    (
        "employee_007_content_categories",
        include_str!("../migrations/007_content_categories.sql"),
    ),
    (
        "employee_008_content_origin",
        include_str!("../migrations/008_content_origin.sql"),
    ),
    // P1-2: the global digital-employee catalog (28 prebuilt ops/office
    // personas). See the migration's own doc comment and `catalog.rs` for the
    // seed/instantiate mechanics built on top of it.
    (
        "employee_009_employee_catalog",
        include_str!("../migrations/009_employee_catalog.sql"),
    ),
];

const MIGRATIONS_MYSQL: &[(&str, &str)] = &[
    (
        "employee_001_init",
        include_str!("../migrations_mysql/001_init.sql"),
    ),
    (
        "employee_002_schedule",
        include_str!("../migrations_mysql/002_schedule.sql"),
    ),
    (
        "employee_003_visibility",
        include_str!("../migrations_mysql/003_visibility.sql"),
    ),
    (
        "employee_004_persona_model",
        include_str!("../migrations_mysql/004_persona_and_model.sql"),
    ),
    (
        "employee_005_dream_rebrand_agent_type",
        include_str!("../migrations_mysql/005_dream_rebrand_agent_type.sql"),
    ),
    (
        "employee_006_grants",
        include_str!("../migrations_mysql/006_grants.sql"),
    ),
    (
        "employee_007_content_categories",
        include_str!("../migrations_mysql/007_content_categories.sql"),
    ),
    (
        "employee_008_content_origin",
        include_str!("../migrations_mysql/008_content_origin.sql"),
    ),
    (
        "employee_009_employee_catalog",
        include_str!("../migrations_mysql/009_employee_catalog.sql"),
    ),
];

/// Run all pending one-employee migrations on the pool's backend. Idempotent.
pub async fn run_one_employee_migrations(pool: &DbPool) -> Result<(), EmployeeError> {
    run_ledgered_migrations(
        pool,
        "_one_migrations",
        MigrationSet {
            sqlite: MIGRATIONS,
            mysql: MIGRATIONS_MYSQL,
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let db = dream_core_db::init_database_memory().await.unwrap();
        run_one_employee_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();
        run_one_employee_migrations(&DbPool::Sqlite(db.pool().clone()))
            .await
            .unwrap();

        for table in ["one_personal_agents", "one_employee_runs"] {
            let exists: bool =
                sqlx::query_scalar("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?")
                    .bind(table)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert!(exists, "table {table} should exist");
        }

        // 002 added schedule columns to one_personal_agents.
        let has_schedule: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('one_personal_agents') WHERE name='schedule'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(has_schedule, "one_personal_agents.schedule column should exist");
        let has_enabled: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('one_personal_agents') WHERE name='schedule_enabled'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(has_enabled, "one_personal_agents.schedule_enabled column should exist");
        let has_next_run: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('one_personal_agents') WHERE name='next_run_at'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(has_next_run, "one_personal_agents.next_run_at column should exist");

        // 004 added the persona + model binding.
        for column in ["assistant_id", "agent_id_override", "model_id", "model"] {
            let exists: bool =
                sqlx::query_scalar("SELECT COUNT(*) > 0 FROM pragma_table_info('one_personal_agents') WHERE name = ?")
                    .bind(column)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert!(exists, "one_personal_agents.{column} column should exist");
        }
    }

    /// Requires a real MySQL 8.0.16+ server via `DREAM_TEST_MYSQL_URL`;
    /// skipped when unset. Also asserts the catalog seed landed exactly 28
    /// reference rows (the seeded persones are byte-identical to SQLite's).
    #[tokio::test]
    async fn migrations_are_idempotent_mysql() {
        let Some(db) = dream_core_db::testing::mysql_test_pool().await else {
            eprintln!("skipping: DREAM_TEST_MYSQL_URL not set");
            return;
        };

        run_one_employee_migrations(&db.pool).await.unwrap();
        run_one_employee_migrations(&db.pool).await.unwrap();

        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _one_migrations WHERE name LIKE 'employee_%'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(applied, MIGRATIONS_MYSQL.len() as i64);

        let seeded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM one_employee_catalog")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(seeded, 28, "catalog seed must land exactly once");

        db.cleanup().await.unwrap();
    }
}
