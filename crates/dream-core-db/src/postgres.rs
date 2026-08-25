//! Additive, parallel Postgres support for the enterprise (dream-en) deployment.
//!
//! This module is entirely independent of [`crate::database::Database`] and
//! its [`sqlx::SqlitePool`] — no shared code, no shared migrations. That is
//! deliberate: most of `database.rs` exists to repair and evolve years of
//! historical SQLite installs (the `users_new` rebuild dance, `PRAGMA
//! legacy_alter_table`, `pragma_table_info` column probes, MCP schema
//! reconciliation). None of that applies to a fresh Postgres deployment,
//! which starts from a clean slate. Sharing the `Database` type would mean
//! either dragging SQLite-only repair logic into a generic abstraction or
//! stubbing it out with dialect branches throughout an already-1000-line
//! file — both riskier than a parallel, additive path that cannot regress
//! the existing SQLite/personal-edition behavior.
//!
//! Scope as of 2026-08-25: only the `users` table is ported (see
//! `migrations_postgres/001_users.sql`), proven end-to-end against a real
//! Postgres instance. The remaining ~21 tables in this crate, plus the 7
//! domain crates that each hand-roll their own SQLite migration runner, are
//! not yet ported — see `docs/e-pg-postgres-support.md` for the full
//! remaining scope and the reasoning behind porting table-by-table instead
//! of replaying all 52 SQLite migrations verbatim.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::error::DbError;

static PG_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations_postgres");

/// A Postgres-backed database handle, parallel to [`crate::database::Database`].
pub struct PgDatabase {
    pool: PgPool,
}

impl PgDatabase {
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Connects to `database_url` and runs the Postgres migrator
/// (`migrations_postgres/`, independent of the SQLite `migrations/` tree).
pub async fn init_database_postgres(database_url: &str) -> Result<PgDatabase, DbError> {
    let pool = PgPoolOptions::new().max_connections(10).connect(database_url).await?;

    PG_MIGRATOR.run(&pool).await?;

    Ok(PgDatabase { pool })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requires a real Postgres instance — set `DREAM_TEST_POSTGRES_URL` to
    /// run it (e.g. `postgres://postgres:test@localhost:55432/dream_test`).
    /// Skipped by default so `cargo test -p dream-core-db` stays hermetic;
    /// this is the one test in the crate that cannot use an in-memory DB.
    #[tokio::test]
    async fn migrates_and_round_trips_a_user_row() {
        let Ok(url) = std::env::var("DREAM_TEST_POSTGRES_URL") else {
            eprintln!("skipping: DREAM_TEST_POSTGRES_URL not set");
            return;
        };

        let db = init_database_postgres(&url).await.expect("connect + migrate");

        sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (id) DO NOTHING",
        )
        .bind("test-user-1")
        .bind("alice")
        .bind("hash")
        .bind(1_756_000_000_000_i64)
        .bind(1_756_000_000_000_i64)
        .execute(db.pool())
        .await
        .expect("insert user");

        let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
            .bind("test-user-1")
            .fetch_one(db.pool())
            .await
            .expect("select user");

        assert_eq!(username, "alice");

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind("test-user-1")
            .execute(db.pool())
            .await
            .expect("cleanup");

        db.close().await;
    }
}
