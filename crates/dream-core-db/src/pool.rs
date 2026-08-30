//! Backend-agnostic connection handle for the enterprise domain crates
//! (`dream-domain-*`). Closed set of two backends: the personal edition and
//! the default enterprise deployment run SQLite; an enterprise deployment can
//! switch the `one_*` tables to MySQL via `DREAM_DATABASE_URL`. A trait-object
//! handle was rejected — sqlx's `Executor` is not object-safe and an
//! any-driver wrapper would drag both drivers into every build anyway — and
//! there will only ever be two backends, so an enum is the smallest surface.
//! If Postgres comes back from dormancy (see [`crate::postgres`]), it is one
//! more variant plus a `migrations_postgres/` tree.

use sqlx::{MySqlPool, SqlitePool};

#[derive(Clone)]
pub enum DbPool {
    Sqlite(SqlitePool),
    MySql(MySqlPool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DbBackend {
    Sqlite,
    MySql,
}

impl DbPool {
    pub fn backend(&self) -> DbBackend {
        match self {
            Self::Sqlite(_) => DbBackend::Sqlite,
            Self::MySql(_) => DbBackend::MySql,
        }
    }

    /// SQLite handle or panic — reserved for personal-edition-only call sites
    /// that a MySQL deployment can never reach. Use sparingly and note next to
    /// the caller why MySQL cannot get there.
    pub fn sqlite(&self) -> &SqlitePool {
        match self {
            Self::Sqlite(pool) => pool,
            _ => panic!("SQLite-only path reached under MySQL"),
        }
    }
}

impl From<SqlitePool> for DbPool {
    fn from(pool: SqlitePool) -> Self {
        Self::Sqlite(pool)
    }
}

impl From<MySqlPool> for DbPool {
    fn from(pool: MySqlPool) -> Self {
        Self::MySql(pool)
    }
}
