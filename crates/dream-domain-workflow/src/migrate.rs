//! Self-managed migration runner for one-workflow tables. Independent ledger
//! (`_one_workflow_migrations`), name-keyed and append-only, mirroring the
//! other `one-*` crates.

use sqlx::SqlitePool;

use crate::error::WorkflowError;

/// Embedded migrations, applied in array order. Append-only: never edit or
/// reorder shipped entries — add a new file instead.
const MIGRATIONS: &[(&str, &str)] = &[("001_init", include_str!("../migrations/001_init.sql"))];

/// Run all pending one-workflow migrations. Idempotent; call once at startup
/// after the upstream database has been initialized.
pub async fn run_one_workflow_migrations(pool: &SqlitePool) -> Result<(), WorkflowError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _one_workflow_migrations (\
             name TEXT PRIMARY KEY,\
             applied_at INTEGER NOT NULL\
         )",
    )
    .execute(pool)
    .await?;

    for (name, sql) in MIGRATIONS {
        let applied: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM _one_workflow_migrations WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await?;
        if applied {
            continue;
        }

        let mut tx = pool.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO _one_workflow_migrations (name, applied_at) VALUES (?, ?)")
            .bind(name)
            .bind(dream_core_common::now_ms())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(migration = name, "one-workflow migration applied");
    }

    Ok(())
}
