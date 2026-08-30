# P3-3 企业版主存储（E-DB）实施方案 —— 目标 MySQL

> **决策（2026-08-31，用户拍板）：企业版主存储用 MySQL，不是 Postgres。**
> 理由：(1) 你们运维栈现成就是 MySQL，不引入第二种 DB 技术；(2) 移植工作量更小——MySQL 占位符
> 跟 SQLite 一样是 `?`（省掉 ~200 处 `?`→`$1` 重写），`json_extract` / `GROUP_CONCAT` 也一样；
> (3) 国内客户 infra 普遍 MySQL 标准化。DB 选型对「对齐竞品」零影响——「对齐」= 能力对齐，
> 数据库是控制台背后的实现细节，客户看不见。
>
> **`DbPool` 抽象 = enum** `DbPool { Sqlite(SqlitePool), MySql(MySqlPool) }`（务实、闭集两后端、改动面最小）。
>
> **Postgres 第一片保留、休眠**（用户 2026-08-31 决定）：`src/postgres.rs`（~100 行）+ `migrations_postgres/001_users.sql`
> （~50 行）+ sqlx `postgres` feature 一共就这点量，删了没意义，留着当「以后要换 PG 的前置工作」。
> `src/mysql.rs` 直接照 `src/postgres.rs` 的形状写（`init_database_mysql(url)` + `sqlx::migrate!("./migrations_mysql")`），
> 两个平行、互不干扰。`sqlx` 两个 feature `["mysql","postgres"]` 都开——只多带一个未用驱动，`dream-core-db` 一个 crate 的事。
> `DbPool` enum 现在是 `{ Sqlite, MySql }`；哪天 PG 回来，enum 加一个变体 + `migrations_postgres/` 补齐即可。
>
> 红线（align §7）：已发布迁移文件内容不可改（sqlx 校验和），只能新增正向迁移；个人版行为逐字节不变；
> 本地不跑 `-p dream-core-app` 全量 e2e。
> 目标 MySQL 版本：**8.0.16+**（CHECK 约束在 8.0.16 起真正强制；`AS new … x = new.x` upsert 别名语法在 8.0.19 起）。

---

## 0. 现状（file:line，DB 无关，从 Postgres 方案继承）

### 0.1 9 个自管迁移 runner（不是文档说的 7——workflow + memory 后加）

| crate | 函数 | ledger 表 | 运行时 `sqlite_master` 依赖 |
|---|---|---|---|
| `dream-domain-org` | `run_one_migrations` `migrate.rs:60` | `_one_migrations` | 无 |
| `dream-domain-sso` | `run_one_sso_migrations` `migrate.rs:27` | `_one_migrations`（共用） | 无 |
| `dream-domain-enterprise` | `run_one_enterprise_migrations` `migrate.rs:37` | `_one_migrations`（共用） | 无 |
| `dream-domain-billing` | `run_one_billing_migrations` `migrate.rs:46` | `_one_migrations`（共用） | 无 |
| `dream-domain-employee` | `run_one_employee_migrations` `migrate.rs:49` | `_one_migrations`（共用） | `#[cfg(test)]` only |
| `dream-domain-platform` | `run_one_platform_migrations` `migrate.rs:35` | `_one_platform_migrations` | 无 |
| `dream-domain-devops` | `run_one_devops_migrations` `migrate.rs:48` | `_one_devops_migrations` | **有**：`backfill_collaboration_tenant_ids` `migrate.rs:101` 运行时查 `sqlite_master` |
| `dream-domain-workflow` | `run_one_workflow_migrations` `migrate.rs:15` | `_one_workflow_migrations` | 无 |
| `dream-domain-memory` | `run_one_memory_migrations` `migrate.rs:15` | `_one_memory_migrations` | 无 |

runner body 模式**完全一致**（范本 `dream-domain-org/src/migrate.rs:60-91`）：
1. `CREATE TABLE IF NOT EXISTS _ledger (name TEXT PRIMARY KEY, applied_at INTEGER NOT NULL)`
2. `for (name, sql) in MIGRATIONS`: `SELECT COUNT(*) > 0 FROM _ledger WHERE name = ?`（`query_scalar::<_, bool>`）
3. 已应用 → skip；否则 `tx.begin()` → `sqlx::raw_sql(sql).execute(&mut *tx)` → `INSERT INTO _ledger (name, applied_at) VALUES (?, ?)` → `tx.commit()`

runner 里的方言点只有：`applied_at INTEGER`（步骤 1，MySQL 要 `BIGINT`）+ `_ledger` 表名（`_one_migrations`
以下划线开头，MySQL 合法）。**占位符 `?` 两边一致，步骤 2/3 不用改**。真正的活是步骤 3 执行的 51 个 `.sql` 文件本体。

### 0.2 全仓 0 处 `sqlx::query!` 编译期宏

所有查询是运行时 `sqlx::query` / `query_as::<_, T>` / `query_scalar`。→ **无 `.sqlx` offline 数据、无编译期 DB 依赖**，
方言问题是纯运行时字符串问题。

### 0.3 迁移调用点 + 域服务构造

- 迁移调用点：`crates/dream-core-app/src/router/routes.rs` ~1360-1443（`#[cfg(feature = "enterprise")]` 块内
  `dream_domain_X::run_..._migrations(services.database.pool())`）+ `routes.rs:2158`（memory 重复调用，幂等）。
- 域服务：`OrgService::new(pool: SqlitePool, ...)` `dream-domain-org/src/service.rs:97`；
  `MemoryService::new(pool: SqlitePool)` `dream-domain-memory/src/service.rs:199`。
  routes.rs 全部 `dream_domain_X::XService::new(services.database.pool().clone(), ...)`。
- `SqlitePool` 全仓 ~284 处 / 75 文件；9 个企业 crate 里粗数：platform ~45、employee ~25、org ~25、devops ~30、
  billing ~18、sso ~10、workflow ~8、memory ~7、enterprise ~6。

---

## 1. MySQL vs SQLite 方言对照（比 Postgres 版短得多）

| 点 | SQLite | **MySQL** | 处理 |
|---|---|---|---|
| **占位符** | `?` | **`?` 一样** | ✅ **不用改**（Postgres 版的 ~200 处重写在这里消失） |
| **`json_extract(col,'$.a.b')`** | ✅ | **一样的函数** | ✅ 不用改（`org/src/service.rs:1805-1815` agent-audit 查询原样） |
| **`GROUP_CONCAT(DISTINCT x)`** | ✅ | **一样** | ✅ 不用改（`billing/src/service.rs:1146`） |
| **`ON CONFLICT(cols) DO UPDATE SET x = excluded.x`** | ✅ | ❌ `INSERT … ON DUPLICATE KEY UPDATE x = new.x`（8.0.19+ 用 `AS new` 别名；旧写法 `x = VALUES(x)` 8.0.20 起弃用） | ~30 处逐条改。**注意**：MySQL 没有「冲突目标列」——它用**任意**唯一键判冲突，所以每张表的唯一约束要设对 |
| **`INSERT OR IGNORE`** | ✅ | `INSERT IGNORE` | ~6 处：`enterprise/src/directory.rs:539`、`platform/src/service.rs:2144,2376,2401`、迁移 `billing_001_init.sql:42`、`org/007_multi_membership.sql:61`、`employee/009_employee_catalog.sql:37` |
| **`INSERT OR REPLACE INTO {table}`**（动态） | ✅ | `REPLACE INTO {table}`（语义一致：按主键/唯一键删了重插） | `org/src/backup.rs:306`——比 Postgres 版简单，`REPLACE` 直接可用 |
| **`strftime('%Y-%m-%d', ts/1000, 'unixepoch')`** | ✅ | `DATE_FORMAT(FROM_UNIXTIME(ts/1000), '%Y-%m-%d')` | `billing/src/service.rs:1124,1575` + `devops/src/dlp_service.rs:397`：抽 `day_bucket_expr(backend) -> &str` |
| **`INTEGER`（时间戳/大数）** | 动态存 i64 | **`INT` 是 32 位会溢出 → `BIGINT`** | 所有 `one_*` 迁移里存 ms 时间戳 / 大计数的 `INTEGER` 列，MySQL 版逐列 `BIGINT` |
| **`REAL` + Rust `f64`** | ✅ | `DOUBLE` | `memory/migrations/001_init.sql:50` `one_memory_items.importance` |
| **`TEXT`（大正文）** | 无长度上限 | **`TEXT` 只有 64KB！** | `spec`（OpenAPI 原文，`devops` API 资产，轻松超 64KB）、`content`（skill 正文 / rag chunk / memory item）、`body`（`one_llm_calls`? 查）、`secrets_json`——这些列 MySQL 版用 **`LONGTEXT`**。短枚举/名字列留 `VARCHAR(255)` 或 `TEXT` |
| **保留字做列名** | ✅ | `key`（`one_config_entries.key`、`one_skill_registry`? 查）**是保留字**；`type`（`one_mcp_registry.type`）8.0 非保留但边界情况要小心；`groups`/`rank`/`system` 若出现也是 | MySQL 版迁移**所有标识符加反引号** \`key\` \`type\`——最省心。或至少 `key` 加 |
| **默认 collation 大小写不敏感** | 敏感 | **`utf8mb4_0900_ai_ci`（8.0 默认）：`WHERE name='API'` 命中 `'api'`** | ⚠️ **行为差异**。影响 config set 别名唯一（`{{config.API.x}}` vs `{{config.api.x}}`）、skill/mcp 同名判断、成员查找。MySQL 版建表/建库用 **`utf8mb4_0900_as_cs`**（大小写敏感）或列级 `COLLATE utf8mb4_bin`。**建库时定，全库一致** |
| **字符集** | UTF-8 | `utf8`（3 字节，存不下 emoji/部分 CJK）vs `utf8mb4`（4 字节） | 建库 `CHARACTER SET utf8mb4`，别用 `utf8` |
| **`CREATE INDEX` on `TEXT`** | ✅ | 要前缀长度 `INDEX idx (col(191))` | grep 确认：**当前迁移没有 TEXT 列上建索引**，一个都没有。少一个坑 |
| **`CHECK(x IN (...))`** | ✅ | 8.0.16+ 真强制 | 目标 8.0.16+，语法一致，不用改 |
| **`AUTOINCREMENT`** | ✅ | `AUTO_INCREMENT` | doc §81 说 2 迁移 + 1 model，企业 crate 迁移里 grep 未见，开工前全仓 grep `AUTOINCREMENT` 定位 |
| **`PRAGMA`/`journal_mode`/`busy_timeout`** | 连接建立 | MySQL 无等价（InnoDB 行锁 + MVCC） | `src/database.rs` 个人版路径专用，MySQL 路径 `src/mysql.rs` 绕开 |
| **`sqlx::raw_sql` 多语句** | ✅ | sqlx MySQL 用 text protocol 支持分号分隔多语句，无参数——现有迁移 `.sql` 无参数，OK | |
| **`sqlite_master`** | ✅ | `SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?` | 只有 `devops` 的 `backfill` 一处（`migrate.rs:101`） |

**净结论**：MySQL 版的真活 = 51 个迁移 `.sql` 最终态手写（`INTEGER→BIGINT`、大正文 `→LONGTEXT`、标识符反引号、
`INSERT OR IGNORE→INSERT IGNORE`、`strftime` 种子语句）+ ~30 处 upsert 改 `ON DUPLICATE KEY UPDATE` +
~6 处 `INSERT OR IGNORE` + ~4 处日期分桶。**没有 ~200 处占位符重写**（Postgres 版的大头）。

---

## 2. `DbPool` enum

### 2.1 `crates/dream-core-db/src/pool.rs`（新），`lib.rs` 导出

```rust
//! 企业域 crate 的后端无关连接句柄。闭集两后端：个人版 + 默认企业版跑 SQLite；
//! 企业运维可通过 DREAM_DATABASE_URL 换 MySQL。trait object 方案否掉——sqlx 的
//! Executor 不 object-safe，Any-driver 丢编译期驱动选择且无条件拉两个驱动，
//! 而且只会有两个后端。enum 是最小面。

use sqlx::{MySqlPool, SqlitePool};

#[derive(Clone)]
pub enum DbPool {
    Sqlite(SqlitePool),
    MySql(MySqlPool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DbBackend { Sqlite, MySql }

impl DbPool {
    pub fn backend(&self) -> DbBackend {
        match self { Self::Sqlite(_) => DbBackend::Sqlite, Self::MySql(_) => DbBackend::MySql }
    }
    /// SQLite 句柄或 panic——留给「Postgres/MySQL 部署下永远不会到」的个人版专用点。
    /// 慎用，且旁边要有注释说明为什么 MySQL 到不了这。
    pub fn sqlite(&self) -> &SqlitePool {
        match self { Self::Sqlite(p) => p, _ => panic!("SQLite-only path reached under MySQL") }
    }
}
impl From<SqlitePool> for DbPool { fn from(p: SqlitePool) -> Self { Self::Sqlite(p) } }
impl From<MySqlPool>  for DbPool { fn from(p: MySqlPool)  -> Self { Self::MySql(p) } }
```

### 2.2 `sqlx` feature 传播

9 个企业 crate 的 `Cargo.toml` 加 `sqlx` 的 `"mysql"` feature（workspace 根默认只有 `["runtime-tokio","sqlite"]`，
`crates/dream-core/Cargo.toml:129`）。个人版二进制多带一个未用的 mysql 驱动（体积 + 编译时间小幅增加，个人版从不连 MySQL）。
**决策：直接加，换零 feature-gate 复杂度**。若不可接受再每 crate 加 `mysql` feature 门控（复杂度显著上升，`enterprise` feature 要传递开）。

### 2.3 `?` vs `?` —— 好消息，enum 摩擦点小得多

MySQL 和 SQLite 占位符都是 `?`，所以**SQL 字符串本身不用改写**。但 `DbPool` enum 不是 `Executor`，
`sqlx::query(sql).bind(a).fetch_*(&self.pool)` 里 `&self.pool` 必须是具体的 `&SqlitePool` 或 `&MySqlPool`。
→ 每个查询点还是要 `match backend` 两臂。方案：

**`dream-core-db` 提供一组薄 helper**（`DbPool` 上的 inherent method 或 extension trait）：
`fetch_optional_as` / `fetch_all_as` / `fetch_one_scalar` / `execute`——每个内部：
```rust
match self {
    DbPool::Sqlite(p) => sqlx::query_as::<sqlx::Sqlite, T>(sql).bind_all(params).fetch_optional(p).await,
    DbPool::MySql(p)  => sqlx::query_as::<sqlx::MySql,  T>(sql).bind_all(params).fetch_optional(p).await,
}
```
参数用自定义枚举 `DbValue::{Text(String), Int(i64), Real(f64), Bool(bool), Bytes(Vec<u8>), Null}`，helper 内部
`for v in params { q = match v { Text(s) => q.bind(s), ... } }`。丧失一点编译期类型安全，换单一 API。
绑定值类型在这个 codebase 里很集中（TEXT/i64/f64/bool），可控。

**新代码约定**：P3-3 之后新写的企业 service 查询一律走 `self.db.*` helper，不再裸 `sqlx::query`。
`dream-core-db/src/lib.rs:1` 已 `#![warn(clippy::disallowed_types)]`，可加规则禁企业 crate 直接用 `sqlx::SqlitePool`。

动态拼 SQL / 变长 `IN (?,?,?)` / `raw_sql` 的少数复杂点（`org/backup.rs`、`memory/service.rs:696` 的
`IN (placeholders)`、`billing` 聚合）逐点手写 `match self.db.backend()` 两臂。

---

## 3. 有序任务分解

### 阶段 1 —— 基础设施（不改任何 service，可独立合入）
1. `dream-core-db`：新增 `src/pool.rs`（`DbPool`/`DbBackend`）、`src/mysql.rs`（`MyDatabase` / `init_database_mysql(url)`——
   照 `src/postgres.rs` 结构，`MySqlPoolOptions::new().max_connections(10).connect(url)` + `sqlx::migrate!("./migrations_mysql")`）、
   `src/dialect.rs`（`DbValue`、`DbPool` 查询 helper、`day_bucket_expr(backend)`）。
2. `dream-core-db/Cargo.toml`：`sqlx` features 加 `"mysql"`（`"postgres"` **保留**——PG 第一片休眠不删）。
   9 个企业 crate `Cargo.toml` 加 `"mysql"`。`src/postgres.rs` / `migrations_postgres/` 原样留着。
3. `crates/dream-core-db/src/migrate_runner.rs`（新）：`run_ledgered_migrations(pool: &DbPool, ledger: &str, migrations: &[EmbeddedMigration])`——
   SQLite 臂逐字节搬现有逻辑；MySQL 臂 `applied_at BIGINT`、`SELECT EXISTS(SELECT 1 FROM \`ledger\` WHERE name = ?)`。
4. `just check-editions` 通过（两版都编译，个人版多带 mysql 驱动）。

### 阶段 2 —— runner「一次解锁 9 个」
5. 每个企业 crate 的 `migrate.rs`：签名 `&SqlitePool` → `&DbPool`；body 换 `dream_core_db::run_ledgered_migrations(...)`；
   `MigrationSet` 按 `pool.backend()` 选 `migrations/` 或 `migrations_mysql/`（ledger key 一致，如 `001_init`）。
6. 为每个 crate 写 `migrations_mysql/*.sql`（最终态手写，逐表读 SQLite 迁移历史确认最终列）。
   **顺序**（FK 依赖）：`org`（`one_tenants`/`one_user_org` 是根）→ `sso` → `enterprise` → `billing`（依赖 enterprise）→
   `platform` → `employee` → `devops`（`backfill` 依赖 `one_user_org`）→ `workflow` → `memory`。
   逐表机械移植 + `INTEGER→BIGINT` / 大正文 `→LONGTEXT` / 标识符反引号 / `INSERT OR IGNORE→INSERT IGNORE` /
   `strftime` 种子语句改写。建库/建表 `DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs`（大小写敏感）。
7. `devops` 的 `backfill_collaboration_tenant_ids`（`migrate.rs:99-147`）：`sqlite_master` → `information_schema.tables`；
   加 `match backend` 分「table exists」判断。
8. `routes.rs` 迁移调用点：阶段 2 先临时 `DbPool::Sqlite(services.database.pool().clone())`，阶段 3 的 `AppServices.db` 就位后改传。
9. 每 crate `migrate.rs` `#[cfg(test)]`：加 `DREAM_TEST_MYSQL_URL` gated 的 MySQL 版幂等测试；SQLite 版测试
   `DbPool::Sqlite(...)` 包一层保持不变。

### 阶段 3 —— service 层 pool 类型迁移（逐 crate，顺序同阶段 2）
10. `AppServices`（`crates/dream-core-app/src/services.rs:28`）：加 `pub db: DbPool` 字段（与现有 `database: Database` 并存）。
    读 `DREAM_DATABASE_URL`（`mysql://…` → MySQL，否则 SQLite 文件）。
11. 逐 crate：`XService { pool: SqlitePool }` → `{ db: DbPool }`；`new(pool: SqlitePool)` → `new(db: DbPool)`；
    每个 `sqlx::query*(...).bind(...).fetch_*(&self.pool)` → `self.db.fetch_*(...)` helper 或手写 match。
    routes.rs 对应 `XService::new(...)` 调用点改传 `services.db.clone()`。
12. `org/src/backup.rs`（动态 SQL）、`memory/src/service.rs:696`（变长 `IN`）、`billing` 聚合（`day_bucket_expr` + `GROUP_CONCAT`）单独处理。
13. 每 crate 迁完：`cargo nextest run -p dream-domain-X`（SQLite 测试全绿）+ MySQL 测试（若 `DREAM_TEST_MYSQL_URL` 可用）。

### 阶段 4 —— 端到端
14. `dream-en/deploy/docker-compose.yml` 加 `mysql:8` 服务（`enterprise` deploy 可选）；`deploy/` 文档写 `DREAM_DATABASE_URL`。
15. 起企业版 `dreamcore` + `dreamcore-admin` 连真 MySQL，跑冒烟：建企业 → 邀请成员 → 场景 → 授权矩阵 → 审批 → 记忆
    （对齐 `handoff-2026-08-30b` 真机验证清单）。
16. `align` 文档 P3-3 标完成 + 进度块。

---

## 4. ⚠️ 开工前必须拍板的一个架构决策（跟 DB 选型无关，PG/MySQL 都有）

`dream-domain-org/src/service.rs:1830-1831`（agent-audit：`FROM messages m JOIN conversations c`）+
`dream-domain-employee/src/service.rs:2019`（`SELECT content FROM messages`）**直接 SQL 引用 `messages` / `conversations`**，
这两张表属于 `dream-core-db` 主 schema，**没有 MySQL 版**（P3-3 只解锁 9 个 `dream-domain-*` crate，不碰主 schema）。
→ 「MySQL 企业部署」在 P3-3 完成后是**混合**的（`one_*` 表在 MySQL，`messages` 等仍在 SQLite），这 2 处查询无表可查。
`users` 的 4 处引用不受影响（`enterprise/src/service.rs:589`、`sso/src/service.rs:891`、`devops/src/service.rs:797`、
`org/src/service.rs:1868`——只要 MySQL 部署也把 `users` 建进 MySQL，同库 JOIN 可行）。

**三选一：**
| | 做法 | 评价 |
|---|---|---|
| **(a)** | 这 2 处改成「MySQL 侧取 audit/employee 行 → 收 conversation_id/msg_id → 从 SQLite repo 批量取 content → Rust 侧拼」 | **推荐**。延续 billing「不跨 crate 联表，Rust 算」先例，范围可控 |
| (b) | 把 `messages` + `conversations`（+ FK 依赖）也纳入 MySQL 移植 | 范围爆炸，等于「企业版对话主存储上 MySQL」，更大的产品决策 |
| (c) | 这 2 个功能在 MySQL 部署下降级/关闭 | 最省事，丢功能 |

---

## 5. 测试策略

- **SQLite 回归**：每 crate 现有 `#[cfg(test)]` 用 `DbPool::Sqlite(init_database_memory().pool().clone())` 包一层，断言不变。
  收尾 `cargo nextest run --workspace`（红线：不跑 `-p dream-core-app` e2e）。
- **MySQL 测试**：`DREAM_TEST_MYSQL_URL` gated（照 `postgres.rs:63` 的 `DREAM_TEST_POSTGRES_URL` 先例改）。默认 CI 跳过。
  `just test-mysql` 起 `mysql:8` 容器 + 设 env + 跑所有 `#[cfg(...mysql...)]` 测试。每 crate 至少：迁移幂等 +
  一条 CRUD 往返 + 一条方言敏感查询（billing 的 day-bucket、config 别名大小写敏感性）。
- **MySQL 测试 harness**：`crates/dream-core-db/src/testing.rs`（`#[cfg(feature = "test-support")]`）`mysql_test_pool()`——
  读 env、建随机库名、跑 migrator、返回 `DbPool::MySql`，`Drop` 时 `DROP DATABASE`。每测试隔离。
- **无 MySQL 时**：`mysql_test_pool()` 返回 `None` → 测试 `return`（不 fail）。

---

## 6. 最大风险与未知

1. **`messages`/`conversations` 跨 schema 引用**（§4）——开工前拍 (a)/(b)/(c)。同时确认 SQLite 部署下这 2 处仍走 SQLite 臂无回归。
2. **MySQL 大小写不敏感 collation**——config set 别名 / skill 名 / mcp 名的唯一性和查找语义。建库用 `utf8mb4_0900_as_cs`。
   逐个核对现有 service 里按 name 查的地方在大小写敏感假设下是否正确（`config_reference_count` 的 `INSTR` 是大小写敏感的——MySQL `INSTR` 随 collation，要留意）。
3. **`TEXT` 64KB**——grep 出所有存「用户可控大正文」的列（`spec`/`content`/`body`/`secrets_json`/`endpoints`），MySQL 版全 `LONGTEXT`。漏一个 = 存长 OpenAPI spec 时静默截断。
4. **`ON DUPLICATE KEY UPDATE` 无冲突目标**——MySQL 按任意唯一键判冲突。逐条核对每个 upsert 对应的表只有一个「预期的」唯一约束，否则会命中错的键。
5. **`DbValue` 参数枚举丢类型安全**——绑错类型编译不拦、跑起来 decode 错。缓解：helper 数量少、绑定类型集中、MySQL 测试覆盖每个 service 主 CRUD。
6. **`sqlx` mysql feature 全局传播**——个人版二进制链接 mysql 驱动（体积 + 编译时间）。建议先接受。
7. **`_one_migrations` 共用 ledger**——org/sso/enterprise/billing/employee 共用。MySQL 版这张表必须在第一个 runner（org）里 `CREATE TABLE IF NOT EXISTS`。顺序在 routes.rs 已保证（org 最先），启动串行调用无并发。
8. **51 个 `.sql` 最终态手写量**——`platform`(10)、`devops`(15)、`org`(13) 累计 ~40 张表要逐个读迁移历史确认最终列。
   `migrations_postgres/001_users.sql` 一张表就花了「读 4 个迁移文件」。可写辅助脚本：每张 `one_*` 表 grep 所有
   `ALTER TABLE <t>` / `CREATE TABLE <t>` 汇总列，人工校对。
9. **`REPLACE INTO` vs `INSERT ... ON DUPLICATE`**——`org/backup.rs:306` 动态拼 `INSERT OR REPLACE` 改 `REPLACE INTO`，
   语义一致（删了重插，触发 `ON DELETE` 级联）——确认备份恢复的表没有会被级联误删的子表。
10. **`AUTOINCREMENT` 的 3 处**（doc §81）——开工前全仓 grep `AUTOINCREMENT` 定位。企业 crate 迁移里没 grep 到，可能在 `dream-core-db` 主迁移（不在本 plan 范围）或 model 层。

---

## 7. 涉及文件清单

**新增**：
- `crates/dream-core-db/src/pool.rs`（`DbPool` / `DbBackend`）
- `crates/dream-core-db/src/mysql.rs`（`init_database_mysql`——照 `src/postgres.rs`）
- `crates/dream-core-db/src/migrate_runner.rs`（`run_ledgered_migrations` / `EmbeddedMigration`）
- `crates/dream-core-db/src/dialect.rs`（`DbValue` / 查询 helper / `day_bucket_expr`）
- `crates/dream-core-db/src/testing.rs`（`mysql_test_pool()`，test-support）
- `crates/dream-domain-{org,sso,enterprise,billing,platform,employee,devops,workflow,memory}/migrations_mysql/*.sql`（~51 文件）

**改**：
- `crates/dream-core-db/src/lib.rs`（导出新模块）
- `crates/dream-core-db/Cargo.toml`（sqlx features 加 `"mysql"`，`"postgres"` 保留）
- **不动**：`src/postgres.rs` / `migrations_postgres/`（PG 第一片休眠，当以后换 PG 的前置工作）
- 9 个 `dream-domain-*/Cargo.toml`（sqlx 加 `"mysql"`）
- 9 个 `dream-domain-*/src/migrate.rs`（`&SqlitePool` → `&DbPool`，body 换 `run_ledgered_migrations`，`MigrationSet` 按 backend 选树）
- 9 个 `dream-domain-*/src/service.rs`（+ 子模块 `directory.rs`/`backup.rs`/`dlp_service.rs`/`provider_channel.rs`/`api_assets.rs`/`catalog.rs`/`rbac.rs` 等）——`pool: SqlitePool` → `db: DbPool`，每个查询点走 helper 或 `match backend`
- `crates/dream-domain-devops/src/migrate.rs`（`backfill` 的 `sqlite_master` → `information_schema`）
- `crates/dream-core-app/src/services.rs`（`AppServices` 加 `db: DbPool`；读 `DREAM_DATABASE_URL`）
- `crates/dream-core-app/src/router/routes.rs`（迁移调用点 + 9 个 `XService::new` 调用点传 `services.db.clone()`）
- `crates/dream-core-app/src/bootstrap/environment.rs` / `bin/admin.rs`（MySQL 连接初始化分支）
- `dream-en/deploy/` compose + 文档（`mysql:8` 服务 + `DREAM_DATABASE_URL`）
- `crates/dream-core-db/docs/e-pg-postgres-support.md`（顶部加一行：「2026-08-31：企业版主存储改走 MySQL（`enterprise-db-p3-3-implementation-plan.zh-CN.md`）；本文件描述的 PG 第一片保留、休眠，当以后换 PG 的前置工作」）

**每 crate `#[cfg(test)]`**——加 MySQL-gated 平行测试。

---

## 8. 实施记录（2026-08-31，阶段 1+2 完成）

阶段 1（基建）与阶段 2（runner + 66 个 MySQL 迁移文件）已完成，等 cargo 解禁后统一编译验证。
真库验证方式：每写完一套 `migrations_mysql/`，用 `scratchpad/replay_mysql.sh <dir> <db>` 在
`dream-mysql-test` 容器（mysql:8.0.46，端口 13306，root/test）里逐文件回放——9 套全部 0 错误通过。

### 实施中新增的方言发现（§1 表格的补充）

| 发现 | 处理 |
|---|---|
| **`SENSITIVE` 是 MySQL 8.0 保留字**（`one_config_entries.sensitive`） | 反引号。真库回放抓到的，文档上没写 |
| **`VALUE`/`SENSITIVE` 之外的 `TRIGGER` 也是保留字**（`one_pipelines.trigger`） | 反引号 |
| 复合唯一键宽度：utf8mb4 下 4~5 列 `VARCHAR(255)` 超过 InnoDB 3072 字节索引上限（`one_resource_grants` 5 列、`one_employee_grants` 4 列） | 这些表的 id 列改 `VARCHAR(191)`（191×4=764/列），单列键保持 `VARCHAR(255)` |
| MySQL TEXT 列要设默认值必须用表达式括号形式 | 所有 `TEXT ... DEFAULT '[]'` 写成 `DEFAULT ('[]')` |
| SQLite 局部唯一索引（`WHERE x IS NOT NULL`） | 平凡映射为普通唯一索引——MySQL 唯一索引天然允许多个 NULL，语义等价 |
| SQLite 的 FK 声明（仅 org 2 处）从未被强制（无 PRAGMA foreign_keys） | MySQL 版降为注释，保持行为对齐 |
| org 007 的表重建舞步 | MySQL 直接 `ALTER TABLE ... DROP PRIMARY KEY, ADD PRIMARY KEY (user_id, tenant_id)`，无需重建 |
| billing 001 的 `strftime('%s','now')` 种子 | `CAST(UNIX_TIMESTAMP() AS UNSIGNED) * 1000` |

### 阶段 2 实际形态（与原方案的差异）

- 迁移文件 **1:1 镜像** SQLite 树的文件粒度（66 个 → 66 个），ledger 键逐一对齐；SQLite 表重建类迁移
  在 MySQL 侧写成等价的直接 ALTER，不是逐句翻译。
- 布尔语义列统一 `TINYINT(1)`（可被 bool 和整数双向 decode）；时间戳/计数一律 `BIGINT`。
- 大正文列（skill content / rag chunk / api asset spec / pipeline log / memory content / prompt）
  统一 `LONGTEXT`。
- `dream-core-db/src/testing.rs` 落地 `mysql_test_pool()`（`DREAM_TEST_MYSQL_URL` gated，
  每测试建随机库 `dream_test_<pid>_<nanos>`，`cleanup()` 显式 DROP）。
- 9 个 runner 全部切到 `run_ledgered_migrations`；devops 的 backfill 双后端（`sqlite_master` /
  `information_schema.tables` 探测）。调用点（routes.rs 生产 20 处 + 各 crate 测试 ~50 处）
  以 `&DbPool::Sqlite(...)` 包装——阶段 3 的 `AppServices.db` 就位后生产点改传 `services.db.clone()`。
