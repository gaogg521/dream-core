# P3-3 Postgres 主存储（E-PG）实施方案

> 目标：企业版可以用 Postgres 做主存储。**已决策：`DbPool` 走 enum**
> `DbPool { Sqlite(SqlitePool), Postgres(PgPool) }`（务实、闭集 2 后端、改动面最小、Rust 惯用）。
> 权威计划：`D:\dream\dream-en\docs\align-openocta-2026-08-29.zh-CN.md` P3-3 行 + §3 末「唯一还没定的：E-PG 的 DbPool 抽象形态」。
> 已有基础：`crates/dream-core-db/docs/e-pg-postgres-support.md`（2026-08-25 第一片，`users` 表已对真实 PG 验证）。
>
> 红线（`align` §7）：已发布迁移文件内容不可改（sqlx 校验和），只能新增正向迁移；个人版行为逐字节不变；本地不跑 `-p dream-core-app` 全量 e2e。

---

## 0. 现状（已核对，file:line）

### 0.1 已经做了的（第一片，`e-pg-postgres-support.md`）
- `crates/dream-core-db/src/postgres.rs` —— `PgDatabase` / `init_database_postgres(url)`：
  `PgPoolOptions::new().max_connections(10).connect(url)` + `sqlx::migrate!("./migrations_postgres")`
  （用 **sqlx 自带 migrator**，不是手写 runner）。导出 `crates/dream-core-db/src/lib.rs:37`。
- `crates/dream-core-db/migrations_postgres/001_users.sql` —— `users` 表按 SQLite 52 迁移后的**最终态**手写一份，
  已对 `postgres:16-alpine` 验证（测试 `postgres.rs:63` `migrates_and_round_trips_a_user_row`，gated on `DREAM_TEST_POSTGRES_URL`，默认跳过）。
- `crates/dream-core-db/Cargo.toml:8` —— `sqlx = { workspace = true, features = ["migrate", "postgres"] }`
  （workspace 默认只有 `["runtime-tokio", "sqlite"]`，`crates/dream-core/Cargo.toml:129`）。
  **只有 `dream-core-db` 拉了 postgres 驱动**，其它 crate 没有。
- `PgDatabase` 与 `Database`（`crates/dream-core-db/src/database.rs:49`，`pool: SqlitePool`）完全独立，零共享代码。

### 0.2 关键已知事实（决定方案形状）
- **全仓 0 处 `sqlx::query!` / `query_as!` / `query_scalar!` 编译期宏**（grep 确认）。所有查询是运行时
  `sqlx::query` / `query_as::<_, T>` / `query_scalar`。→ **没有 `.sqlx` offline 数据、没有编译期 DB 依赖**，
  `?` vs `$1` 是纯运行时字符串问题。
- **`INTEGER` 陷阱**：SQLite 的动态 `INTEGER` 存毫秒时间戳没问题；PG `INTEGER` 是 int4（max ~2.1e9），
  毫秒时间戳溢出。且 Rust 层全部 model 成 `i64`，sqlx 的 PG decode 是**精确类型匹配** —— `i64` 读 `INT4` 列
  直接 `ColumnDecode` 报错。`migrations_postgres/001_users.sql:35-45` 已把每个时间戳列改 `BIGINT`。
  → 所有 `one_*` 迁移里的 `INTEGER`（时间戳、计数）在 PG 版必须逐列判断 `BIGINT`。
- **`REAL` 同类陷阱**：`one_memory_items.importance REAL`（`memory/migrations/001_init.sql:50`），Rust 是 `f64`
  （`MemoryService` `importance: f64`）。PG `REAL` = float4，`f64` decode 会 mismatch → PG 版要 `DOUBLE PRECISION`。
- **9 个自管迁移 runner**（不是 7 —— 那份 doc 写于 P2-1/P2-2 之前；workflow 和 memory 后来又加了 2 个）：

  | crate | 函数 | ledger 表 | 前缀 | 运行时 `sqlite_master` 依赖 |
  |---|---|---|---|---|
  | `dream-domain-org` | `run_one_migrations` `migrate.rs:60` | `_one_migrations` | 无（`001_`…） | 无（仅 `#[cfg(test)]`） |
  | `dream-domain-sso` | `run_one_sso_migrations` `migrate.rs:27` | `_one_migrations`（共用） | `sso_` | 无 |
  | `dream-domain-enterprise` | `run_one_enterprise_migrations` `migrate.rs:37` | `_one_migrations`（共用） | `enterprise_` | 无 |
  | `dream-domain-billing` | `run_one_billing_migrations` `migrate.rs:46` | `_one_migrations`（共用） | `billing_` | 无 |
  | `dream-domain-employee` | `run_one_employee_migrations` `migrate.rs:49` | `_one_migrations`（共用） | `employee_`? | `#[cfg(test)]` `:94` |
  | `dream-domain-platform` | `run_one_platform_migrations` `migrate.rs:35` | `_one_platform_migrations` | `001_`… | 无 |
  | `dream-domain-devops` | `run_one_devops_migrations` `migrate.rs:48` | `_one_devops_migrations` | `001_`… | **有**：`backfill_collaboration_tenant_ids` `migrate.rs:101` 运行时查 `sqlite_master` 判断 `one_user_org` 是否存在 |
  | `dream-domain-workflow` | `run_one_workflow_migrations` `migrate.rs:15` | `_one_workflow_migrations` | `001_`… | 无 |
  | `dream-domain-memory` | `run_one_memory_migrations` `migrate.rs:15` | `_one_memory_migrations` | `001_`… | 无 |

  runner body 模式**完全一致**（以 `dream-domain-org/src/migrate.rs:60-91` 为范本）：
  1. `CREATE TABLE IF NOT EXISTS _ledger (name TEXT PRIMARY KEY, applied_at INTEGER NOT NULL)`
  2. `for (name, sql) in MIGRATIONS`: `SELECT COUNT(*) > 0 FROM _ledger WHERE name = ?` （`query_scalar::<_, bool>`）
  3. 已应用 → skip；否则 `tx.begin()` → `sqlx::raw_sql(sql).execute(&mut *tx)` → `INSERT INTO _ledger (name, applied_at) VALUES (?, ?)` → `tx.commit()`

  → runner 里的 SQLite 特有点只有：`?` 占位符（步骤 2、3）、`applied_at INTEGER`（步骤 1，PG 要 `BIGINT`）。
  **runner 本身不查 `sqlite_master`**（doc §57-69 的表述过时）。真正难的是步骤 3 执行的 51 个 `.sql` 文件本体。

- 迁移调用点：`crates/dream-core-app/src/router/routes.rs` ~1360-1443，每个都在 `#[cfg(feature = "enterprise")]`
  块里 `dream_domain_X::run_..._migrations(services.database.pool())`（`routes.rs:1408,1421,1434` 等）。
  另一处 `routes.rs:2158`（memory 重复调用，幂等）。
- 域服务构造：`OrgService::new(pool: SqlitePool, ...)` `dream-domain-org/src/service.rs:97-98`；
  `MemoryService::new(pool: SqlitePool)` `dream-domain-memory/src/service.rs:199`。
  routes.rs 里全部 `dream_domain_X::XService::new(services.database.pool().clone(), ...)`。
- `SqlitePool` 全仓 284 处 / 75 文件；9 个企业 crate 里：platform ~45、employee ~25、org ~25、devops ~30、
  billing ~18、sso ~10、workflow ~8、memory ~7、enterprise ~6（`grep -c` 粗数，含注释/测试）。
- **`ON CONFLICT (...) DO UPDATE SET x = excluded.x`**：SQLite 和 PG **语法一致**，~30 处（billing/platform/org/enterprise/employee/devops），
  **不用改**。这是最大的好消息 —— upsert 是重灾区但恰好可移植。

### 0.3 方言不兼容点清单（运行时 SQL，已 grep 定位）
| 方言点 | SQLite | Postgres | 位置 |
|---|---|---|---|
| 占位符 | `?` | `$1 $2 …` | **所有** `query`/`query_as`/`query_scalar` bind 处（9 crate 数百处） |
| `INSERT OR IGNORE` | ✅ | `INSERT ... ON CONFLICT DO NOTHING` | `dream-domain-enterprise/src/directory.rs:539`；`platform/src/service.rs:2144,2376,2401`；迁移 `billing_001_init.sql:42`、`org/007_multi_membership.sql:61`、`employee/009_employee_catalog.sql:37` |
| `INSERT OR REPLACE` | ✅ | `INSERT ... ON CONFLICT (...) DO UPDATE SET ...` | `dream-domain-org/src/backup.rs:306`（动态拼 `INSERT OR REPLACE INTO {table}`，备份恢复用，遍历所有 one_ 表） |
| `strftime('%Y-%m-%d', ts/1000, 'unixepoch')` | ✅ | `to_char(to_timestamp(ts/1000.0), 'YYYY-MM-DD')` | `dream-domain-billing/src/service.rs:1124,1575`；`devops/src/dlp_service.rs:397`；迁移 `billing_001_init.sql:43`（`CAST(strftime('%s','now') AS INTEGER)*1000`） |
| `GROUP_CONCAT(DISTINCT x)` | ✅ | `string_agg(DISTINCT x, ',')` | `dream-domain-billing/src/service.rs:1146` |
| `json_extract(col, '$.a.b')` | ✅ | `col::jsonb #>> '{a,b}'` 或 `->>` | `dream-domain-org/src/service.rs:1805-1815`（agent-audit 查询，多个 `json_extract`） |
| `COUNT(*) > 0` → `bool` | ✅ `query_scalar::<_,bool>` | PG `int8 > int4` → `bool`，sqlx 能 decode，但保险起见改 `EXISTS(SELECT 1 ...)` | 9 个 runner + 若干 service |
| `INTEGER`（时间戳/大数） | 动态存 i64 | 必须 `BIGINT` | **所有** `one_*` 迁移 |
| `REAL` + Rust `f64` | ✅ | `DOUBLE PRECISION` | `memory/migrations/001_init.sql:50` |
| `AUTOINCREMENT` | ✅ | `BIGSERIAL` / `GENERATED ... AS IDENTITY` | doc §81 说 2 迁移 + 1 model（需 grep 定位，企业 crate 里未见，可能在 `dream-core-db` 主迁移） |
| `PRAGMA` / `journal_mode` / `busy_timeout` | 连接建立 | PG 无等价（MVCC，no-op） | `crates/dream-core-db/src/database.rs`（个人版路径，PG 路径 `postgres.rs` 已绕开） |
| `CAST(x AS INTEGER)` on text-ish | 宽松 | 严格；`::bigint` | 零散 |

---

## 1. `DbPool` enum 定义

### 1.1 位置：`crates/dream-core-db/src/pool.rs`（新文件），`lib.rs` 导出

```rust
//! The backend-agnostic connection handle for enterprise domain crates.
//!
//! Closed set of exactly two backends: personal edition and the default
//! enterprise deployment run on SQLite; an enterprise operator may opt into
//! Postgres via `DREAM_DATABASE_URL`. A trait object was rejected — sqlx's
//! `Executor` is not object-safe in the way we'd need, `Any`-driver loses
//! compile-time driver selection and pulls both drivers unconditionally,
//! and only two backends will ever exist. An enum is the smallest surface.

use sqlx::{PgPool, SqlitePool};

#[derive(Clone)]
pub enum DbPool {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DbBackend { Sqlite, Postgres }

impl DbPool {
    pub fn backend(&self) -> DbBackend {
        match self { Self::Sqlite(_) => DbBackend::Sqlite, Self::Postgres(_) => DbBackend::Postgres }
    }
    /// SQLite handle or panic — for the personal-edition-only call sites that
    /// are never reached under a Postgres deployment. Use sparingly and only
    /// where a comment explains why Postgres can't get here.
    pub fn sqlite(&self) -> &SqlitePool {
        match self { Self::Sqlite(p) => p, Self::Postgres(_) => panic!("SQLite-only path reached under Postgres") }
    }
}

impl From<SqlitePool> for DbPool { fn from(p: SqlitePool) -> Self { Self::Sqlite(p) } }
impl From<PgPool>     for DbPool { fn from(p: PgPool)     -> Self { Self::Postgres(p) } }
```

### 1.2 `sqlx` feature 传播
每个持有 `DbPool` 的 crate 的 `Cargo.toml` 需要 `sqlx` 带 `postgres`：
- 方案 A（推荐）：这些 crate **不直接依赖 sqlx 的 postgres 特性**，而是只 `use dream_core_db::DbPool`，
  所有需要 `PgPool` 的类型都从 `dream-core-db` re-export。这样只有 `dream-core-db` 一个 crate 拉 postgres 驱动。
  但域服务里有大量 `sqlx::query(...).bind(...).fetch_*(&pool)` —— `fetch_*` 需要 `&self.pool` 是
  `Executor<Database = X>`。`DbPool` enum 不是 `Executor`。→ **必须在每个查询处 match**（见 §3），
  match 的每个臂 `sqlx::query::<Sqlite>` / `sqlx::query::<Postgres>` —— 后者需要 `sqlx` 的 `postgres` feature
  在**该 crate**可见。所以 9 个 crate 的 `Cargo.toml` 都要加 `sqlx.features = ["postgres"]`（additive，个人版构建也会编进 postgres 驱动 —— 体积增加，但个人版从不连 PG。可接受，或用 feature gate，见 §7 风险）。
- 决策：**9 个企业 crate 的 `Cargo.toml` 直接加 `sqlx` 的 `postgres` feature**。个人版二进制多带一个未用驱动，
  换来零 feature-gate 复杂度。

---

## 2. 迁移 runner 的 backend 感知抽象

### 2.1 每个 crate 的 `run_*_migrations(pool: &SqlitePool)` → `run_*_migrations(pool: &DbPool)`

runner body 是模板化的，抽出一个共享 helper 到 `dream-core-db`：

`crates/dream-core-db/src/migrate_runner.rs`（新）：
```rust
use crate::pool::{DbBackend, DbPool};

/// One embedded migration: (ledger key, raw SQL). The SQL is the
/// backend-appropriate body — see `MigrationSet`.
pub struct EmbeddedMigration { pub name: &'static str, pub sql: &'static str }

/// Runs the shared name-keyed ledger pattern the 9 `one-*` crates use,
/// dialect-aware. `ledger` is the crate's ledger table name.
pub async fn run_ledgered_migrations(
    pool: &DbPool,
    ledger: &str,
    migrations: &[EmbeddedMigration],
) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Sqlite(p) => run_sqlite(p, ledger, migrations).await,
        DbPool::Postgres(p) => run_postgres(p, ledger, migrations).await,
    }
}
```
- `run_sqlite`：现有逻辑逐字节搬过来（`?` 占位、`applied_at INTEGER`）。
- `run_postgres`：`$1`/`$2` 占位、`applied_at BIGINT`、`SELECT EXISTS(SELECT 1 FROM ledger WHERE name = $1)`。
  「table exists」判断（若将来需要）：`SELECT to_regclass($1) IS NOT NULL`（比 `information_schema.tables` 更简）。
- 每个 crate 的 `migrate.rs` 变成：
  ```rust
  pub async fn run_one_org_migrations(pool: &DbPool) -> Result<(), OrgError> {
      dream_core_db::run_ledgered_migrations(pool, "_one_migrations", ORG_MIGRATIONS()).map_err(Into::into).await
  }
  ```

### 2.2 迁移 SQL 本体 —— 每 crate 一个 `migrations_postgres/` 树

`MigrationSet` 按 backend 选 body：
```rust
fn org_migrations(backend: DbBackend) -> Vec<EmbeddedMigration> {
    match backend {
        DbBackend::Sqlite => vec![
            EmbeddedMigration { name: "001_init", sql: include_str!("../migrations/001_init.sql") },
            // ...
        ],
        DbBackend::Postgres => vec![
            EmbeddedMigration { name: "001_init", sql: include_str!("../migrations_postgres/001_init.sql") },
            // ...
        ],
    }
}
```
- **ledger key 必须一致**（`001_init` 等）——同一逻辑迁移，只是方言不同。
- Postgres `.sql` 是**最终态**手写（照 `migrations_postgres/001_users.sql` 的方式：读完该表所有 SQLite 迁移，
  写一份 `CREATE TABLE`），不逐个复刻 SQLite 迁移历史（fresh install 无历史数据）。
- 好消息：多数 `one_*` 迁移是干净的 `CREATE TABLE IF NOT EXISTS (TEXT/INTEGER/REAL...) + UNIQUE(...) + CREATE INDEX IF NOT EXISTS`
  （见 `platform/migrations/003_resource_grants.sql` 范例），机械移植 + 改 `INTEGER→BIGINT` / `REAL→DOUBLE PRECISION` /
   处理 §0.3 的少数 `INSERT OR IGNORE` / `strftime` 种子语句即可。
- **`CHECK` 约束、partial unique index（`WHERE ... IS NOT NULL`）、`DEFAULT`** PG 语法一致（doc §19-20 已验证）。

### 2.3 不建议的替代：运行时方言翻译
在 runner 里对 `.sql` 字符串做正则替换（`?`→`$n`、`INTEGER`→`BIGINT`、`strftime`→…）—— 脆弱、
对 `CHECK`/触发器/复杂表达式不可靠、难测。**否决**。只对 runner 自己那 3 行模板 SQL 做手写双版本。

---

## 3. `?` vs `$1` —— service 层查询的策略（本 plan 的 crux）

### 3.1 事实
- sqlx 运行时 `query("...")` 的占位符是**驱动特定**的：SQLite = `?`（或 `?NNN`），Postgres = `$1`。**不通用**。
- 无编译期宏 → 编译不会拦，跑起来才 `error: near "?"` 或参数绑定错位。
- `sqlx::QueryBuilder` 的 `.push_bind()` 会**按驱动生成正确占位符** —— 但要求把每条 SQL 改写成 builder 形式，
  数百处，工作量巨大且可读性下降。

### 3.2 决策：**「新代码 Postgres-first + 运行时占位符改写 helper」双管**

**(a) 占位符改写 helper（覆盖存量的主力）**
`dream-core-db` 提供：
```rust
/// Rewrite `?` placeholders to `$1,$2,…` for Postgres; identity for SQLite.
/// Only touches `?` that are not inside string literals. Every enterprise
/// service query string goes through this at call time.
pub fn pg_placeholders(sql: &str, backend: DbBackend) -> std::borrow::Cow<'_, str>;
```
- SQLite：直接返回原串（零成本 `Cow::Borrowed`）。
- Postgres：状态机扫描，跳过 `'...'` / `"..."` 内的 `?`，其余按序替换成 `$1,$2,...`。
- 每个企业 service 的查询包一层：`sqlx::query(&db.rewrite(SQL)).bind(a).bind(b)...` ——
  但 `.bind` + `.fetch_*` 仍需要知道是 `Sqlite` 还是 `Postgres` executor。

**(b) 每个查询点 match backend（不可避免的机械改造）**
`DbPool` 不是 `Executor`，所以：
```rust
// 之前：
let row: Option<(String,)> = sqlx::query_as("SELECT x FROM t WHERE id = ?")
    .bind(id).fetch_optional(&self.pool).await?;
// 之后（helper 封装）：
let row: Option<(String,)> = self.db.query_as_opt::<(String,)>(
    "SELECT x FROM t WHERE id = ?", |q| q.bind(id)
).await?;
```
在 `dream-core-db` 提供一组薄封装（`DbPool` 上的 inherent method 或 extension trait）：
`fetch_optional_as` / `fetch_all_as` / `fetch_one_scalar` / `execute` —— 每个内部
`match self { Sqlite(p) => { let sql = sql; sqlx::query_as::<Sqlite,_>(sql)...bind_fn... }, Postgres(p) => { let sql = pg_placeholders(sql); sqlx::query_as::<Postgres,_>(&sql)... } }`。
`bind_fn: FnOnce(Query) -> Query` 闭包传参数 —— 但 `Query<Sqlite>` 和 `Query<Postgres>` 是不同类型，
闭包没法泛型。**这是 enum 方案的核心摩擦点。** 两个解法：
- **B1**：闭包接受 `&mut dyn FnMut`… 不行（类型不同）。
- **B2（推荐）**：参数用 `Vec<DbValue>`（自定义枚举 `DbValue::Text(String) | Int(i64) | Real(f64) | Bool(bool) | Null`），
  helper 内部 `for v in params { q = match v { Text(s) => q.bind(s), ... } }`。丧失一点类型安全，
  换来单一 API。绑定值类型在这个 codebase 里很集中（TEXT / i64 / f64 / bool），可控。
- **B3**：不做通用 helper，逐查询手写 `match self.db.backend()` 两臂。最直白、最类型安全、最啰嗦（数百处 ×2）。

**决策**：B2（`DbValue` + 薄 helper）覆盖 90% 的简单查询；B3（手写 match）用于动态拼 SQL / `QueryBuilder` /
`IN (...)` 变长占位 / `raw_sql` 的少数复杂点（如 `org/backup.rs`、`memory/service.rs:696` 的 `IN (placeholders)`、
`billing` 的 strftime 聚合）。

**(c) 新代码约定**：P3-3 之后新写的企业 service 查询一律走 `self.db.*` helper，不再裸 `sqlx::query`。
clippy `disallowed_types`（`dream-core-db/src/lib.rs:1` 已在用 `#![warn(clippy::disallowed_types)]`）可加规则
禁止企业 crate 直接 `sqlx::SqlitePool`。

### 3.3 方言函数（strftime / GROUP_CONCAT / json_extract / INSERT OR）
这些不是占位符问题，helper 修不了。逐点 `match backend` 出不同 SQL 片段：
- `billing/src/service.rs:1124,1575` + `devops/src/dlp_service.rs:397`：`day_bucket_expr(backend) -> &str`
  返回 `strftime(...)` 或 `to_char(to_timestamp(...), 'YYYY-MM-DD')`。
- `billing/src/service.rs:1146`：`GROUP_CONCAT(DISTINCT model)` → `string_agg(DISTINCT model, ',')`。
- `org/src/service.rs:1805-1815`（agent-audit）：`json_extract(m.content,'$.a.b')` →
  `m.content::jsonb #>> '{a,b}'`（需确认 `m.content` 列在 PG 版是 `TEXT` 还是 `JSONB`；建议 PG 版存 `JSONB`，
  这条查询才快 —— 但 messages 表属 `dream-core-db` 主 schema，不在本 plan 的 9 crate 范围，见 §5）。
- `enterprise/src/directory.rs:539` + `platform/src/service.rs:2144,2376,2401`：`INSERT OR IGNORE` →
  `INSERT ... ON CONFLICT (<uniq cols>) DO NOTHING`（每处要知道对应唯一约束的列）。
- `org/src/backup.rs:306`：动态 `INSERT OR REPLACE INTO {table}` → PG `INSERT ... ON CONFLICT (<pk>) DO UPDATE SET ...`
  需要每张表的主键列（backup 已经知道 `column_list`，加一个 `pk_columns(table)` 映射）。

---

## 4. 有序任务分解

### 阶段 1 —— 基础设施（不改任何 service，可独立合入）
1. `crates/dream-core-db`：新增 `src/pool.rs`（`DbPool` / `DbBackend`）、`src/migrate_runner.rs`
   （`run_ledgered_migrations` + `EmbeddedMigration`）、`src/dialect.rs`（`pg_placeholders`、`DbValue`、
   `DbPool` 上的 `fetch_*` / `execute` helper）。`lib.rs` 导出。单测：`pg_placeholders` 覆盖字符串字面量里的 `?`、
   连续 `?`、无 `?`。
2. `crates/dream-core-db/Cargo.toml`：已有 `postgres`，无改动。9 个企业 crate 的 `Cargo.toml`：`sqlx` 加 `"postgres"` feature。
3. `just check-editions` 通过（个人版 + 企业版都编译，多带 pg 驱动）。

### 阶段 2 —— runner「一次解锁 N 个」
4. 每个企业 crate 的 `migrate.rs`：签名 `&SqlitePool` → `&DbPool`；body 换成调 `run_ledgered_migrations`；
   `MigrationSet` 按 `pool.backend()` 选 `migrations/` 或 `migrations_postgres/`。
5. 为每个 crate 写 `migrations_postgres/*.sql`（最终态手写，逐表读 SQLite 迁移；`INTEGER→BIGINT`、`REAL→DOUBLE PRECISION`、
   `INSERT OR IGNORE`/`strftime` 种子语句改写）。**顺序**（按 FK 依赖 + 已有先例）：
   `org`（`one_tenants` / `one_user_org` 是几乎所有其它表的 FK 根）→ `sso` → `enterprise` → `billing`（依赖 enterprise，
   `billing_001` grandfather `one_enterprises`）→ `platform` → `employee` → `devops`（`backfill` 依赖 `one_user_org`）→
   `workflow` → `memory`。
6. `devops` 的 `backfill_collaboration_tenant_ids`（`migrate.rs:99-147`）：`sqlite_master` → `to_regclass`；
   `UPDATE ... SET tenant_id = (SELECT ...)` 的 `format!` 拼串，PG 语法基本一致，只是子查询里没有 `?`，可直接跑；
   加 `match backend` 分「table exists」判断。
7. `routes.rs` 迁移调用点（`~1360-1443`、`2158`）：`services.database.pool()` → 传 `DbPool`
   （阶段 3 的 `AppServices.db` 就位后）。阶段 2 先临时 `DbPool::Sqlite(services.database.pool().clone())`。
8. 每个 crate 的 `migrate.rs` `#[cfg(test)]`：加一个 `#[cfg_attr(not(pg), ignore)]` 的 PG 版幂等测试，
   gated on `DREAM_TEST_POSTGRES_URL`（照 `postgres.rs:63`）。SQLite 版测试保持不变（`DbPool::Sqlite` 包一层）。

### 阶段 3 —— service 层 pool 类型迁移（逐 crate）
9. `AppServices`（`crates/dream-core-app/src/services.rs:28`）：加 `pub db: DbPool` 字段（与现有 `database: Database`
   并存 —— 个人版路径继续用 `Database`，企业路径两者都填）。`init_database_postgres` 或 `init_database` 后构造。
   环境变量：`DREAM_DATABASE_URL`（`postgres://…` → PG，否则 SQLite 文件）。
10. 逐 crate（顺序同阶段 2）：`XService { pool: SqlitePool }` → `{ db: DbPool }`；`new(pool: SqlitePool)` → `new(db: DbPool)`；
    每个 `sqlx::query*(...).bind(...).fetch_*(&self.pool)` → `self.db.fetch_*(...)` helper（B2）或手写 match（B3）。
    routes.rs 对应 `XService::new(...)` 调用点改传 `services.db.clone()`。
11. `dream-domain-org/src/backup.rs`（动态 SQL，B3 手写）、`memory/src/service.rs:696`（变长 `IN`，B3）、
    `billing` 聚合查询（strftime/GROUP_CONCAT，B3 + §3.3 方言片段）单独处理。
12. 每 crate 迁完：`cargo nextest run -p dream-domain-X`（SQLite 测试全绿）+ PG 测试（若 `DREAM_TEST_POSTGRES_URL` 可用）。

### 阶段 4 —— 端到端
13. `docker compose` 加 `postgres:16` 服务（可选，enterprise deploy）；`deploy/` 文档写 `DREAM_DATABASE_URL` 配置。
14. 起企业版 `dreamcore` + `dreamcore-admin` 连真实 PG，跑一遍冒烟：建企业 → 邀请成员 → 场景 → 授权矩阵 →
    审批 → 记忆（对齐 `handoff-2026-08-30b` 的真机验证清单）。
15. `align` 文档 P3-3 标完成 + 进度块。

---

## 5. 范围边界（明确不做 / 后续）
- **`dream-core-db` 主 schema 的 21 张表**（conversations / messages / providers / assistants / teams / ...）——
  本 plan 原则上**不碰**。企业版若要「全 PG」，那是 P3-3 之后的独立大件（doc §57）。本 plan 只解锁 9 个 `dream-domain-*` crate。
  → 意味着：一个「PG 企业部署」在本 plan 完成后是**混合**的 —— 大部分 `dream-core-db` 表仍在 SQLite，`one_*` 表在 PG。
- **跨 schema 引用（已 grep 全部定位，这是本 plan 最硬的约束）**：9 个企业 crate 里有 6 个直接 SQL 引用
  `dream-core-db` 拥有的表：
  - `users`：`enterprise/src/service.rs:589`（`one_enterprise_members m LEFT JOIN users u`）、
    `sso/src/service.rs:891`（`JOIN users`）、`devops/src/service.rs:797`（`SELECT username FROM users`）、
    `org/src/service.rs:1868`（`JOIN users u ON u.id = uo.user_id`）。
    → `users` **已经**有 PG 版（`migrations_postgres/001_users.sql`），所以只要企业 PG 部署也把 `users` 建进 PG（本来就是第一片做的），这 4 处**同库 JOIN，可行 ✅**。
  - `messages` / `conversations`：`org/src/service.rs:1830-1831`（agent-audit：`FROM messages m JOIN conversations c`）、
    `employee/src/service.rs:2019`（`SELECT content FROM messages`）。
    → `messages` / `conversations` **没有** PG 版。这两处在 PG 部署下**无表可查**。
  **裂缝 = `messages` + `conversations` 两张表。** 三个选项（开工前必须拍板）：
  (a) 把这 2 处查询改成「PG 取 audit/employee 行 → 收集 conversation_id/msg_id → 从 SQLite 侧 repo 批量取 content → Rust 拼」，
      延续 billing「不跨 crate 联表，Rust 侧算」的先例。**推荐**，范围可控。
  (b) 把 `messages` + `conversations`（+ 其 FK 依赖）也纳入本 plan 的 `migrations_postgres` 移植 —— 范围明显扩大，
      且 `messages` 是热表（会话流水），迁它等于「企业版对话主存储也上 PG」，是更大的产品决策。
  (c) 这 2 个功能（agent-audit 的工具级明细、employee 的某处 message 读取）在 PG 部署下暂时降级/关闭 —— 最省事但丢功能。
- 双二进制方案（每后端一个 build）—— 已否决（决策：enum）。
- `dream-core-db` 的 `PRAGMA`/WAL 并发故事（E3）—— PG 不需要，`postgres.rs` 已绕开。

---

## 6. 测试策略
- **SQLite 回归**：每 crate 现有 `#[cfg(test)]` 用 `DbPool::Sqlite(init_database_memory().pool().clone())` 包一层，
  断言不变。`cargo nextest run --workspace`（收尾，红线 6 不跑 `-p dream-core-app` e2e）。
- **PG 测试**：`DREAM_TEST_POSTGRES_URL` gated（已有先例 `postgres.rs:63`）。默认 CI 跳过。
  提供一个 `just test-pg` 起 `postgres:16-alpine` 容器 + 设 env + 跑所有 `#[cfg(...pg...)]` 测试。
  每 crate 至少：migration 幂等 + 一条 CRUD 往返 + 一条方言敏感查询（billing 的 day-bucket、org 的 audit json_extract）。
- **PG 测试 harness**：新增 `crates/dream-core-db/src/testing.rs`（`#[cfg(feature = "test-support")]`）：
  `pg_test_pool()` —— 读 env、建一个随机 schema、跑 migrator、返回 `DbPool::Postgres`，`Drop` 时 `DROP SCHEMA CASCADE`。
  让每个测试隔离。
- **无 PG 时**：`pg_test_pool()` 返回 `None` → 测试 `return`（不 fail）。

---

## 7. 最大风险与未知

1. **跨 schema 引用 —— `messages` / `conversations`（已全量 grep，见 §5）**。`org/src/service.rs:1830-1831`
   （agent-audit）+ `employee/src/service.rs:2019` 直接 `FROM messages` / `JOIN conversations`，这两张表无 PG 版。
   `users` 的 4 处引用不受影响（`users` 已有 PG 版）。**必须在动工前在 §5 的 (a)/(b)/(c) 里拍板。** 建议 (a)（Rust 侧拼）。
   同时确认：SQLite 部署下这两处仍走 SQLite executor（`DbPool::Sqlite` 臂），无回归。
2. **`sqlx` postgres feature 全局传播**（§1.2）。加进 9 crate 的 `Cargo.toml` 后，个人版二进制也链接 libpq/pg 驱动
   （体积 + 编译时间）。若不可接受 → 每 crate 加 `postgres` feature 门控（`#[cfg(feature = "postgres")]` 包住
   `DbPool::Postgres` 臂）—— 复杂度显著上升，且 `enterprise` feature 要传递开 `postgres`。**建议先接受体积代价**。
3. **`DbValue` 参数枚举丢类型安全**（§3.2 B2）。绑错类型编译不拦、跑起来 decode 错。缓解：helper 数量少、
   绑定类型集中（TEXT/i64/f64/bool/NULL）、PG 测试覆盖每个 service 的主 CRUD。
4. **51 个 `.sql` 的最终态手写量**。虽然多数简单，但 `platform`（10 迁移）、`devops`（15 迁移）、`org`（13 迁移）
   累计 ~40 张表要逐个读迁移历史确认最终列。`migrations_postgres/001_users.sql` 一张表就花了「读 4 个迁移文件」。
   工作量真实。可写一个辅助脚本：对每张 `one_*` 表，`grep` 所有 `ALTER TABLE <t>` / `CREATE TABLE <t>` 汇总列，人工校对。
5. **`ON CONFLICT` 的 `excluded` 在 PG 需要显式指定冲突目标**。SQLite 允许 `ON CONFLICT DO UPDATE`（无目标，取任意唯一约束），
   PG 必须 `ON CONFLICT (col1, col2) DO UPDATE`。现有代码大多已写了目标（`routes.rs` grep 显示
   `ON CONFLICT(enterprise_id) DO UPDATE` 等），但要逐条核对无「裸 ON CONFLICT」。
6. **`_one_migrations` 共用 ledger**（org/sso/enterprise/billing/employee 共用一张 `_one_migrations`）。
   PG 版这张表必须在**第一个** runner（org）里 `CREATE TABLE IF NOT EXISTS`，后续 runner 依赖它已存在。
   顺序在 routes.rs 已保证（org 最先），但 `run_ledgered_migrations` 的 `CREATE TABLE IF NOT EXISTS _ledger`
   每个 runner 都跑一次（幂等），OK。注意 PG 的 `CREATE TABLE IF NOT EXISTS` 并发下有已知竞态 —— 启动串行调用，无并发，OK。
7. **`sqlx::raw_sql` 在 PG 下的多语句行为**。SQLite `raw_sql` 执行分号分隔的多语句；PG 的 `raw_sql` 也支持
   （simple query protocol），但**不能带参数**且事务语义不同。现有迁移 `.sql` 无参数，OK。但 PG 下
   `raw_sql` 里若有 `CREATE INDEX CONCURRENTLY` 会因在事务里而失败 —— 迁移里别用 `CONCURRENTLY`（本来也没有）。
8. **时间/数值默认值**：SQLite `DEFAULT 0` 对 `BIGINT` OK；`DEFAULT (strftime(...))` 这种表达式默认值
   （若有）PG 语法不同。grep `DEFAULT (` 确认。
9. **`DbPool: Clone` 的连接池语义**。`SqlitePool` / `PgPool` clone 是 `Arc` 共享同一池，OK —— 与现在
   `services.database.pool().clone()` 到处传的语义一致。
10. **未知：`AUTOINCREMENT` 的 3 处**（doc §81）。企业 crate 迁移里没 grep 到，可能在 `dream-core-db` 主迁移
    （不在本 plan 范围）或 model 层。开工前 grep `AUTOINCREMENT` 全仓确认落点。

---

## 8. 涉及文件清单

**新增：**
- `crates/dream-core-db/src/pool.rs` —— `DbPool` / `DbBackend`。
- `crates/dream-core-db/src/migrate_runner.rs` —— `run_ledgered_migrations` / `EmbeddedMigration`。
- `crates/dream-core-db/src/dialect.rs` —— `pg_placeholders` / `DbValue` / `DbPool` 查询 helper。
- `crates/dream-core-db/src/testing.rs` —— `pg_test_pool()`（test-support）。
- `crates/dream-domain-{org,sso,enterprise,billing,platform,employee,devops,workflow,memory}/migrations_postgres/*.sql`
  —— 每 crate 一套最终态 PG 迁移（共 ~51 文件）。

**改：**
- `crates/dream-core-db/src/lib.rs` —— 导出新模块。
- `crates/dream-core-db/Cargo.toml` —— （已有 postgres，无改）。
- 9 个 `dream-domain-*/Cargo.toml` —— `sqlx` 加 `"postgres"` feature。
- 9 个 `dream-domain-*/src/migrate.rs` —— `&SqlitePool` → `&DbPool`，body 换 `run_ledgered_migrations`，`MigrationSet` 按 backend 选树。
- 9 个 `dream-domain-*/src/service.rs`（+ `directory.rs` / `backup.rs` / `dlp_service.rs` / `provider_channel.rs` /
  `api_assets.rs` / `catalog.rs` / `retrieval.rs` / `rbac.rs` 等子模块）—— `pool: SqlitePool` → `db: DbPool`，
  每个查询点走 helper 或 `match backend`。
- `crates/dream-domain-devops/src/migrate.rs` —— `backfill_collaboration_tenant_ids` 的 `sqlite_master` → `to_regclass`。
- `crates/dream-core-app/src/services.rs` —— `AppServices` 加 `db: DbPool`；构造逻辑读 `DREAM_DATABASE_URL`。
- `crates/dream-core-app/src/router/routes.rs` —— 迁移调用点 + 9 个 `XService::new` 调用点传 `services.db.clone()`。
- `crates/dream-core-app/src/bootstrap/environment.rs` / `bin/admin.rs` —— PG 连接初始化分支。
- `deploy/` compose + 文档 —— `postgres` 服务 + `DREAM_DATABASE_URL`。

**每 crate 的 `#[cfg(test)]`** —— 加 PG-gated 平行测试。
