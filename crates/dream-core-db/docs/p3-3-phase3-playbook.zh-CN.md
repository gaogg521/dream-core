# P3-3 阶段 3 操作手册 —— service 层 pool → DbPool（逐 crate）

> 阶段 1/2 已完成并提交（`eb5024e`、`114bec8`），66 个 MySQL 迁移已在真 mysql:8.0.46
> 回放 0 错误。本文档是阶段 3 的执行手册：**需要 cargo 反馈窗口**——改的是函数签名，
> 个人版构建也编译 memory/workflow/employee/devops 四个 crate，不能在别人打包时动。

## 前置（一次性）

1. `dream-core-app/src/services.rs` 的 `AppServices`（~:28）加字段
   `pub db: dream_core_db::DbPool`，构造处解析 `DREAM_DATABASE_URL`：
   `mysql://` 开头 → `init_database_mysql(url).await?` 包成 `DbPool::MySql(pool)`；
   否则 `DbPool::Sqlite(database.pool().clone())`。与现有 `database: Database` 并存
   （主 schema 永远 SQLite）。
2. routes.rs 生产调用点（20 处）把 `&DbPool::Sqlite(services.database.pool().clone())`
   换成 `&services.db`；9 个 `XService::new(...)` 调用点改传 `services.db.clone()`。
   bootstrap/environment.rs、bin/admin.rs 的 MySQL 初始化分支照 `mysql.rs` 文档。

## 每 crate 的机械套路（以 memory 为范本先做，编译通过后复制节奏）

对 `XService`：

1. `pool: SqlitePool` 字段 → `db: DbPool`；`new(pool: SqlitePool, ...)` → `new(db: DbPool)`；
   内部所有 `&self.pool` → `&self.db`。
2. 查询点映射表（`dream_core_db` 的 helper，见 `dialect.rs`）：

   | 原写法 | 新写法 |
   |---|---|
   | `sqlx::query(sql).bind(a).bind(b).execute(&self.pool).await?` | `self.db.execute(sql, &db_params![a, b]).await?` |
   | `sqlx::query_as::<_, T>(sql).bind(..).fetch_optional(&self.pool)` | `self.db.fetch_optional_as::<T>(sql, &db_params![..])` |
   | `sqlx::query_as::<_, T>(..).fetch_all(&self.pool)` | `self.db.fetch_all_as::<T>(..)` |
   | `sqlx::query_scalar::<_, i64>(sql).bind(..).fetch_one(&self.pool)` | `self.db.fetch_one_scalar::<i64>(sql, &db_params![..])` |
   | 同上 fetch_optional / fetch_all | fetch_optional_scalar / fetch_all_scalar |
   | `pool.begin()` + 多条语句 + commit | `self.db.begin()` → `DbTx::execute/execute_raw/commit`（dialect.rs） |

3. 类型口径注意（遇到才处理，别预先改）：
   - `query_scalar::<_, bool>`（`SELECT COUNT(*) > 0` / `EXISTS`）：MySQL 侧 BIGINT decode
     成 bool 不可靠 → SQL 改 `SELECT COUNT(*)` 收 `i64`，Rust 侧 `> 0`。
   - 绑定 `bool`：`DbValue::Bool` 两个后端都 OK。
   - `u32`/`i32`：`DbValue::from` 已覆盖。
   - `FromRow` 结构体字段全部是 String/i64/f64/bool/Vec<u8>/Option<_> 的不用动；
     含自定义 newtype（实现过 `Encode<Sqlite>` 的）要看有没有 `Encode<MySql>`——
     目前只有 `TimestampMs = i64` 别名，无风险。
4. 复杂点单列（方案 §3.12）：`org/backup.rs` 动态拼 SQL（`INSERT OR REPLACE` →
   `REPLACE INTO`，注意确认无级联误删）、`memory/service.rs:696` 变长 `IN (?,?,?)`
   （两臂手拼）、`billing` 的 `day_bucket_expr(backend, "started_at")` + `GROUP_CONCAT`
   （两个后端语法一致，确认 collation 即可）。
5. 测试：crate 内 `#[cfg(test)]` 的 `XService::new(...)` 调用点传
   `DbPool::Sqlite(init_database_memory().await.unwrap().pool().clone())`；
   MySQL-gated 用例用 `dream_core_db::testing::mysql_test_pool()`（模式见各
   `migrate.rs` 的 `migrations_are_idempotent_mysql`）。
6. 每 crate 完成即跑 `cargo nextest run -p dream-domain-X`（SQLite 全绿）+
   `DREAM_TEST_MYSQL_URL=mysql://root:test@localhost:13306/dream_test cargo nextest
   run -p dream-domain-X -- --skip sqlite`（或直接全跑，未设 env 的 MySQL 用例自动 skip）。

## 顺序与体量（SqlitePool 引用数）

`memory`(7) → `workflow`(8) → `enterprise`(6) → `sso`(10) → `billing`(18) → `org`(25) →
`employee`(25) → `platform`(45) → `devops`(30)。先做 3 个小的建立编译反馈基线，
再平推剩下 6 个。

## 决策 (a) 的两处拆查（随 org / employee 一并做）

- `dream-domain-org/src/service.rs:1830-1831`（agent-audit）：MySQL 臂先在 MySQL 取
  audit 行（`one_audit_logs` 联 `one_user_org`），收集 conversation_id/msg_id 集合，
  再从 SQLite repo（`IConversationRepository`）批量取 messages/conversations，
  Rust 侧拼接 DTO。SQLite 臂维持原联表不动。
- `dream-domain-employee/src/service.rs:2019`（`SELECT content FROM messages`）：同法，
  MySQL 侧取 `one_employee_runs` 行 → SQLite repo 按 conversation_id/turn_id 批量取
  content → 拼。两处都要补「跨库 id 不存在」的容错（取不到就跳过该条，不 fail）。

## 冒烟（阶段 4，全部 crate 完成后）

`docker compose --profile mysql up -d`（dream-en/deploy，见 `deploy/MYSQL.md`）：
建企业 → 邀请成员 → 场景 → 授权矩阵 → 审批 → 记忆（对齐 handoff-2026-08-30b 清单）。
收尾：`just check-editions` + `cargo nextest run --workspace`（红线：不跑
`-p dream-core-app` e2e）+ align 文档 P3-3 标完成。
