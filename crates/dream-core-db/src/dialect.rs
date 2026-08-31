//! Query helpers over [`crate::pool::DbPool`] plus the handful of dialect
//! fragments where MySQL and SQLite disagree.
//!
//! `DbPool` is an enum, not an `Executor`, so every query point needs a
//! backend arm. These helpers centralize the arms: callers pass the SQL text
//! (placeholders are `?` on both backends) and a `&[DbValue]` parameter list,
//! and get one API for execute / fetch that works on either pool. The cost is
//! losing compile-time parameter typing (values ride as [`DbValue`]); the
//! mitigations are that bind types in this codebase are concentrated in
//! TEXT/i64/f64/bool and that every service's main CRUD is covered by
//! MySQL-gated tests (see the P3-3 implementation plan, §2.3 and §5).
//!
//! New enterprise service queries after P3-3 go through these helpers instead
//! of raw `sqlx::query` so the backend dispatch has one home.

use sqlx::mysql::MySqlArguments;
use sqlx::sqlite::SqliteArguments;
use sqlx::{Arguments, MySql, Sqlite};

use crate::pool::{DbBackend, DbPool};

/// A dynamically typed bind parameter. `Null` binds as `Option::<String>::None`,
/// which encodes a NULL on both backends.
#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    Text(String),
    Int(i64),
    Real(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    Null,
}

impl DbValue {
    fn push_sqlite(&self, args: &mut SqliteArguments<'_>) {
        match self {
            DbValue::Text(v) => args.add(v.clone()).expect("encode DbValue"),
            DbValue::Int(v) => args.add(*v).expect("encode DbValue"),
            DbValue::Real(v) => args.add(*v).expect("encode DbValue"),
            DbValue::Bool(v) => args.add(*v).expect("encode DbValue"),
            DbValue::Bytes(v) => args.add(v.clone()).expect("encode DbValue"),
            DbValue::Null => args.add(Option::<String>::None).expect("encode DbValue"),
        }
    }

    fn push_mysql(&self, args: &mut MySqlArguments) {
        match self {
            DbValue::Text(v) => args.add(v.clone()).expect("encode DbValue"),
            DbValue::Int(v) => args.add(*v).expect("encode DbValue"),
            DbValue::Real(v) => args.add(*v).expect("encode DbValue"),
            DbValue::Bool(v) => args.add(*v).expect("encode DbValue"),
            DbValue::Bytes(v) => args.add(v.clone()).expect("encode DbValue"),
            DbValue::Null => args.add(Option::<String>::None).expect("encode DbValue"),
        }
    }
}

impl From<&str> for DbValue {
    fn from(v: &str) -> Self {
        DbValue::Text(v.to_owned())
    }
}
impl From<&String> for DbValue {
    fn from(v: &String) -> Self {
        DbValue::Text(v.clone())
    }
}

impl From<String> for DbValue {
    fn from(v: String) -> Self {
        DbValue::Text(v)
    }
}

impl From<i64> for DbValue {
    fn from(v: i64) -> Self {
        DbValue::Int(v)
    }
}

impl From<i32> for DbValue {
    fn from(v: i32) -> Self {
        DbValue::Int(i64::from(v))
    }
}

impl From<u32> for DbValue {
    fn from(v: u32) -> Self {
        DbValue::Int(i64::from(v))
    }
}

impl From<f64> for DbValue {
    fn from(v: f64) -> Self {
        DbValue::Real(v)
    }
}

impl From<bool> for DbValue {
    fn from(v: bool) -> Self {
        DbValue::Bool(v)
    }
}

impl From<Vec<u8>> for DbValue {
    fn from(v: Vec<u8>) -> Self {
        DbValue::Bytes(v)
    }
}

impl From<Option<bool>> for DbValue {
    fn from(v: Option<bool>) -> Self {
        match v {
            Some(v) => DbValue::Bool(v),
            None => DbValue::Null,
        }
    }
}

impl From<&bool> for DbValue {
    fn from(v: &bool) -> Self {
        DbValue::Bool(*v)
    }
}

impl From<Option<&str>> for DbValue {
    fn from(v: Option<&str>) -> Self {
        match v {
            Some(v) => DbValue::Text(v.to_owned()),
            None => DbValue::Null,
        }
    }
}

impl From<&Option<String>> for DbValue {
    fn from(v: &Option<String>) -> Self {
        match v {
            Some(v) => DbValue::Text(v.clone()),
            None => DbValue::Null,
        }
    }
}

impl From<Option<String>> for DbValue {
    fn from(v: Option<String>) -> Self {
        match v {
            Some(v) => DbValue::Text(v),
            None => DbValue::Null,
        }
    }
}

impl From<Option<i64>> for DbValue {
    fn from(v: Option<i64>) -> Self {
        match v {
            Some(v) => DbValue::Int(v),
            None => DbValue::Null,
        }
    }
}

impl From<Option<f64>> for DbValue {
    fn from(v: Option<f64>) -> Self {
        match v {
            Some(v) => DbValue::Real(v),
            None => DbValue::Null,
        }
    }
}

/// Builds a `Vec<DbValue>` from expressions convertible via `From`:
/// `db_params![tenant_id, "admin", Some(name.clone())]`.
#[macro_export]
macro_rules! db_params {
    ($($v:expr),* $(,)?) => {
        vec![$($crate::DbValue::from($v)),*]
    };
}

impl DbPool {
    /// Executes a statement, returning the affected row count.
    pub async fn execute(&self, sql: &str, params: &[DbValue]) -> Result<u64, sqlx::Error> {
        match self {
            DbPool::Sqlite(pool) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                let rows = sqlx::query_with(sql, args).execute(pool).await?.rows_affected();
                Ok(rows)
            }
            DbPool::MySql(pool) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                let rows = sqlx::query_with(sql, args).execute(pool).await?.rows_affected();
                Ok(rows)
            }
        }
    }

    /// Runs a query returning at most one row decoded via `FromRow` on both
    /// backends. `T` must be a plain struct (derive `sqlx::FromRow`) whose
    /// field types decode identically from SQLite and MySQL rows.
    pub async fn fetch_optional_as<T>(&self, sql: &str, params: &[DbValue]) -> Result<Option<T>, sqlx::Error>
    where
        T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow>
            + for<'r> sqlx::FromRow<'r, sqlx::mysql::MySqlRow>
            + Send
            + Unpin,
    {
        match self {
            DbPool::Sqlite(pool) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                sqlx::query_as_with::<Sqlite, T, SqliteArguments>(sql, args)
                    .fetch_optional(pool)
                    .await
            }
            DbPool::MySql(pool) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                sqlx::query_as_with::<MySql, T, MySqlArguments>(sql, args)
                    .fetch_optional(pool)
                    .await
            }
        }
    }

    /// Runs a query returning exactly one row decoded via `FromRow` on both
    /// backends (errors if no row).
    pub async fn fetch_one_as<T>(&self, sql: &str, params: &[DbValue]) -> Result<T, sqlx::Error>
    where
        T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow>
            + for<'r> sqlx::FromRow<'r, sqlx::mysql::MySqlRow>
            + Send
            + Unpin,
    {
        match self.fetch_optional_as::<T>(sql, params).await? {
            Some(v) => Ok(v),
            None => Err(sqlx::Error::RowNotFound),
        }
    }

    /// Runs a query returning every row decoded via `FromRow` on both backends.
    pub async fn fetch_all_as<T>(&self, sql: &str, params: &[DbValue]) -> Result<Vec<T>, sqlx::Error>
    where
        T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow>
            + for<'r> sqlx::FromRow<'r, sqlx::mysql::MySqlRow>
            + Send
            + Unpin,
    {
        match self {
            DbPool::Sqlite(pool) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                sqlx::query_as_with::<Sqlite, T, SqliteArguments>(sql, args)
                    .fetch_all(pool)
                    .await
            }
            DbPool::MySql(pool) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                sqlx::query_as_with::<MySql, T, MySqlArguments>(sql, args)
                    .fetch_all(pool)
                    .await
            }
        }
    }

    /// Runs a query returning at most one scalar value.
    pub async fn fetch_optional_scalar<S>(&self, sql: &str, params: &[DbValue]) -> Result<Option<S>, sqlx::Error>
    where
        S: for<'r> sqlx::Decode<'r, Sqlite>
            + sqlx::Type<Sqlite>
            + for<'r> sqlx::Decode<'r, MySql>
            + sqlx::Type<MySql>
            + Send
            + Unpin,
    {
        match self {
            DbPool::Sqlite(pool) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                sqlx::query_scalar_with::<Sqlite, S, SqliteArguments>(sql, args)
                    .fetch_optional(pool)
                    .await
            }
            DbPool::MySql(pool) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                sqlx::query_scalar_with::<MySql, S, MySqlArguments>(sql, args)
                    .fetch_optional(pool)
                    .await
            }
        }
    }

    /// Runs a query returning exactly one scalar value (errors if no row).
    pub async fn fetch_one_scalar<S>(&self, sql: &str, params: &[DbValue]) -> Result<S, sqlx::Error>
    where
        S: for<'r> sqlx::Decode<'r, Sqlite>
            + sqlx::Type<Sqlite>
            + for<'r> sqlx::Decode<'r, MySql>
            + sqlx::Type<MySql>
            + Send
            + Unpin,
    {
        match self.fetch_optional_scalar::<S>(sql, params).await? {
            Some(v) => Ok(v),
            None => Err(sqlx::Error::RowNotFound),
        }
    }

    /// Runs a query returning one scalar per row.
    pub async fn fetch_all_scalar<S>(&self, sql: &str, params: &[DbValue]) -> Result<Vec<S>, sqlx::Error>
    where
        S: for<'r> sqlx::Decode<'r, Sqlite>
            + sqlx::Type<Sqlite>
            + for<'r> sqlx::Decode<'r, MySql>
            + sqlx::Type<MySql>
            + Send
            + Unpin,
    {
        match self {
            DbPool::Sqlite(pool) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                sqlx::query_scalar_with::<Sqlite, S, SqliteArguments>(sql, args)
                    .fetch_all(pool)
                    .await
            }
            DbPool::MySql(pool) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                sqlx::query_scalar_with::<MySql, S, MySqlArguments>(sql, args)
                    .fetch_all(pool)
                    .await
            }
        }
    }
}

/// SQL fragment bucketing a millisecond timestamp expression into UTC days —
/// the one date-math spot where the two backends' functions diverge
/// (`billing` usage stats and `devops` DLP reports). Wrap the column/alias
/// expression, e.g. `day_bucket_expr(backend, "started_at")`.
pub fn day_bucket_expr(backend: DbBackend, ts_expr: &str) -> String {
    match backend {
        DbBackend::Sqlite => format!("strftime('%Y-%m-%d', ({ts_expr}) / 1000, 'unixepoch')"),
        DbBackend::MySql => format!("DATE_FORMAT(FROM_UNIXTIME(({ts_expr}) / 1000), '%Y-%m-%d')"),
    }
}

/// Opens a transaction on whichever backend the pool holds. The two
/// transaction types are distinct, so the closure receives an opaque
/// [`DbTx`] that routes `execute` through the same backend.
pub struct DbTx<'p> {
    inner: DbTxInner<'p>,
}

enum DbTxInner<'p> {
    Sqlite(sqlx::Transaction<'p, Sqlite>),
    MySql(sqlx::Transaction<'p, MySql>),
}

impl DbTx<'_> {
    /// The backend this transaction is running on (for dialect-specific SQL).
    pub fn backend(&self) -> DbBackend {
        match &self.inner {
            DbTxInner::Sqlite(_) => DbBackend::Sqlite,
            DbTxInner::MySql(_) => DbBackend::MySql,
        }
    }

    pub async fn execute(&mut self, sql: &str, params: &[DbValue]) -> Result<u64, sqlx::Error> {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                let rows = sqlx::query_with(sql, args).execute(&mut **tx).await?.rows_affected();
                Ok(rows)
            }
            DbTxInner::MySql(tx) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                let rows = sqlx::query_with(sql, args).execute(&mut **tx).await?.rows_affected();
                Ok(rows)
            }
        }
    }

    /// Fetches at most one row via `FromRow` on both backends.
    pub async fn fetch_optional_as<T>(&mut self, sql: &str, params: &[DbValue]) -> Result<Option<T>, sqlx::Error>
    where
        T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow>
            + for<'r> sqlx::FromRow<'r, sqlx::mysql::MySqlRow>
            + Send
            + Unpin,
    {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                sqlx::query_as_with::<Sqlite, T, SqliteArguments>(sql, args)
                    .fetch_optional(&mut **tx)
                    .await
            }
            DbTxInner::MySql(tx) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                sqlx::query_as_with::<MySql, T, MySqlArguments>(sql, args)
                    .fetch_optional(&mut **tx)
                    .await
            }
        }
    }

    /// Fetches every row via `FromRow` on both backends.
    pub async fn fetch_all_as<T>(&mut self, sql: &str, params: &[DbValue]) -> Result<Vec<T>, sqlx::Error>
    where
        T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow>
            + for<'r> sqlx::FromRow<'r, sqlx::mysql::MySqlRow>
            + Send
            + Unpin,
    {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                sqlx::query_as_with::<Sqlite, T, SqliteArguments>(sql, args)
                    .fetch_all(&mut **tx)
                    .await
            }
            DbTxInner::MySql(tx) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                sqlx::query_as_with::<MySql, T, MySqlArguments>(sql, args)
                    .fetch_all(&mut **tx)
                    .await
            }
        }
    }

    /// Fetches at most one scalar value.
    pub async fn fetch_optional_scalar<S>(&mut self, sql: &str, params: &[DbValue]) -> Result<Option<S>, sqlx::Error>
    where
        S: for<'r> sqlx::Decode<'r, Sqlite>
            + sqlx::Type<Sqlite>
            + for<'r> sqlx::Decode<'r, MySql>
            + sqlx::Type<MySql>
            + Send
            + Unpin,
    {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                sqlx::query_scalar_with::<Sqlite, S, SqliteArguments>(sql, args)
                    .fetch_optional(&mut **tx)
                    .await
            }
            DbTxInner::MySql(tx) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                sqlx::query_scalar_with::<MySql, S, MySqlArguments>(sql, args)
                    .fetch_optional(&mut **tx)
                    .await
            }
        }
    }

    /// Fetches exactly one scalar value (errors if no row).
    pub async fn fetch_one_scalar<S>(&mut self, sql: &str, params: &[DbValue]) -> Result<S, sqlx::Error>
    where
        S: for<'r> sqlx::Decode<'r, Sqlite>
            + sqlx::Type<Sqlite>
            + for<'r> sqlx::Decode<'r, MySql>
            + sqlx::Type<MySql>
            + Send
            + Unpin,
    {
        match self.fetch_optional_scalar::<S>(sql, params).await? {
            Some(v) => Ok(v),
            None => Err(sqlx::Error::RowNotFound),
        }
    }

    /// Fetches one scalar per row.
    pub async fn fetch_all_scalar<S>(&mut self, sql: &str, params: &[DbValue]) -> Result<Vec<S>, sqlx::Error>
    where
        S: for<'r> sqlx::Decode<'r, Sqlite>
            + sqlx::Type<Sqlite>
            + for<'r> sqlx::Decode<'r, MySql>
            + sqlx::Type<MySql>
            + Send
            + Unpin,
    {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => {
                let mut args = SqliteArguments::default();
                for p in params {
                    p.push_sqlite(&mut args);
                }
                sqlx::query_scalar_with::<Sqlite, S, SqliteArguments>(sql, args)
                    .fetch_all(&mut **tx)
                    .await
            }
            DbTxInner::MySql(tx) => {
                let mut args = MySqlArguments::default();
                for p in params {
                    p.push_mysql(&mut args);
                }
                sqlx::query_scalar_with::<MySql, S, MySqlArguments>(sql, args)
                    .fetch_all(&mut **tx)
                    .await
            }
        }
    }

    /// Executes a raw multi-statement SQL string inside the transaction
    /// (migration bodies; no parameters, which is what keeps multi-statement
    /// legal on both protocols).
    pub async fn execute_raw(&mut self, sql: &str) -> Result<u64, sqlx::Error> {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => Ok(sqlx::raw_sql(sql).execute(&mut **tx).await?.rows_affected()),
            DbTxInner::MySql(tx) => Ok(sqlx::raw_sql(sql).execute(&mut **tx).await?.rows_affected()),
        }
    }

    pub async fn commit(self) -> Result<(), sqlx::Error> {
        match self.inner {
            DbTxInner::Sqlite(tx) => tx.commit().await,
            DbTxInner::MySql(tx) => tx.commit().await,
        }
    }

    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        match self.inner {
            DbTxInner::Sqlite(tx) => tx.rollback().await,
            DbTxInner::MySql(tx) => tx.rollback().await,
        }
    }
}

impl DbPool {
    /// Begins a transaction on the current backend.
    pub async fn begin(&self) -> Result<DbTx<'_>, sqlx::Error> {
        match self {
            DbPool::Sqlite(pool) => Ok(DbTx {
                inner: DbTxInner::Sqlite(pool.begin().await?),
            }),
            DbPool::MySql(pool) => Ok(DbTx {
                inner: DbTxInner::MySql(pool.begin().await?),
            }),
        }
    }
}
