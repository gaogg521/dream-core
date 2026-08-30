# E-PG: PostgreSQL support for the enterprise deployment

> **2026-08-31 更新：企业版主存储改走 MySQL**，不是 Postgres（用户拍板——运维栈现成是 MySQL，
> 移植工作量更小：MySQL 占位符跟 SQLite 一样是 `?`，省掉 ~200 处 `?`→`$1` 重写）。
> P3-3 的实施方案见 [`enterprise-db-p3-3-implementation-plan.zh-CN.md`](enterprise-db-p3-3-implementation-plan.zh-CN.md)。
> **本文件描述的 PG 第一片（`src/postgres.rs` + `migrations_postgres/001_users.sql` + sqlx `postgres` feature）
> 保留、休眠**——一共约 150 行 + 1 个迁移文件，删了没意义，当「以后若要换 PG」的前置工作留着。

Status as of 2026-08-25: **first slice landed and verified against a real
Postgres instance**; full parity is not done. This doc is the honest map of
what exists, what doesn't, and why the chosen approach looks the way it does.

## Why not touch `Database`/`SqlitePool` in place

`crates/dream-core-db/src/database.rs` (~1000 lines) is not just "connect +
run migrations". Most of its bulk is repair logic for **historical SQLite
installs**: the migration-002 legacy data normalization, migration-042's
`users_new`/rebuild-and-swap dance (SQLite has no `ALTER TABLE ... ADD
CONSTRAINT`, so column/constraint changes require a full table rebuild),
`PRAGMA legacy_alter_table` toggling, `pragma_table_info` column-existence
probes, and MCP-server schema reconciliation for a specific class of
partially-migrated installs.

None of that applies to a fresh Postgres deployment — there is no legacy
Postgres data to repair, because there's no existing Postgres install at
all. Rewriting `Database` to be generic over `Sqlite`/`Postgres` would mean
either (a) dragging all of that repair logic into a dialect-abstracted
form it was never designed for, or (b) littering an already-large file with
`if postgres { } else { }` branches around code that's structurally
SQLite-only. Both are higher-risk than adding a **separate, parallel**
module that shares nothing but the crate's existing `DbError` type (which
was already dialect-agnostic — it wraps `sqlx::Error`/`MigrateError`
directly, no SQLite-specific variants).

This is why `src/postgres.rs` + `migrations_postgres/` exist independently
of `src/database.rs` + `migrations/`. **Zero risk to the existing SQLite /
personal-edition path** — nothing in the SQLite path was touched.

## What's actually done

- `PgDatabase` / `init_database_postgres(url)` in `src/postgres.rs`: opens a
  `PgPool` (via `PgPoolOptions`), runs `sqlx::migrate!("./migrations_postgres")`
  (sqlx's own migrator, same mechanism `dream-core-db` already uses for
  SQLite — **not** the hand-rolled `sqlite_master`-checking runner the 7
  domain crates use; see below).
- `migrations_postgres/001_users.sql`: the `users` table, ported from the
  SQLite schema as it exists today **after all 52 migrations** — verified by
  reading every file that touches `users` (001, 025, 042, 046; grepped the
  full migration set to confirm no others do) rather than porting migration
  001 and guessing at later drift.
- Verified end-to-end against a real `postgres:16-alpine` container (WSL2
  Docker, see `crates/dream-core-db/src/postgres.rs` test
  `migrates_and_round_trips_a_user_row`, gated on `DREAM_TEST_POSTGRES_URL`
  so it's skipped by default and doesn't require Postgres in normal CI):
  migration runs, table/indexes/CHECK constraints materialize exactly as
  written (confirmed via `psql \d users`), insert + read + delete round-trip
  succeeds.
- `sqlx`'s `postgres` feature added to `dream-core-db`'s `Cargo.toml` only
  (not the workspace root) — no other crate's build picks up the Postgres
  driver.

## What's explicitly NOT done (the real remaining scope)

- **21 of 22 tables** in `dream-core-db`'s own schema (conversations,
  messages, providers, mcp_servers, assistants, teams, projects, etc.).
  `users` was chosen first because nearly everything else FKs to it.
- **The 7 domain crates' own migrations** (`dream-domain-{org, sso,
  enterprise, billing, platform, devops, employee}`, 51 more `.sql` files
  total). These don't use `sqlx::migrate!()` at all — each hand-rolls a
  migration runner that checks `sqlite_master` for a crate-specific ledger
  table and executes raw `.sql` via `sqlx::raw_sql`. That runner pattern
  itself needs a Postgres-aware variant (checking `information_schema.tables`
  or `pg_catalog` instead of `sqlite_master`) before any of those crates'
  migrations can run against Postgres — a different, separate piece of work
  from what's done here.
- **The pool-type threading problem**: 63 files across the workspace
  reference `SqlitePool` directly in type signatures (repository structs,
  `RouterState`s, function params). None of that has been touched. Getting
  a real Postgres-backed `dreamcore`/`dreamcore-admin` running end-to-end
  requires either a `DbPool` enum/trait abstraction threaded through all of
  those, or running two entirely separate binary builds — an architecture
  decision that hasn't been made yet and is bigger than this session's slice.
- **Known dialect-specific call sites** (catalogued by grep, not yet
  addressed): `strftime` (11 files, mostly migrations plus
  `dream-domain-billing/src/service.rs`'s usage-by-day bucketing query),
  `sqlite_master` (~15 files — the 7 domain migration runners plus
  `dream-core-db` internals/tests), `AUTOINCREMENT` (2 migrations + 1
  model), `PRAGMA`/`journal_mode`/`busy_timeout` (connection setup, central
  to the WAL-mode concurrency story from E3 — Postgres has no equivalent
  pragma set, this is a no-op or MVCC-equivalent-config on that side).

## Why table-by-table, not "port all 52 migrations"

45 of `dream-core-db`'s 52 SQLite migrations are incremental fixes on top of
001 — bug fixes, renamed columns, one-off data backfills accumulated over
the personal edition's history (e.g. "fix cursor agent CLI command", "clear
internal aion CLI command override", "drop unused assistant definition
fields"). A fresh Postgres install has no data to backfill and doesn't need
to replay that history — it only needs the **current final schema state**.
Porting incrementally, one currently-needed table at a time, verified
against real Postgres each time, is safer than either (a) mechanically
translating all 52 files 1:1 (most of which are irrelevant or actively
wrong for a fresh install) or (b) guessing at the final shape of a table
without reading every migration that touches it.

## Suggested next slice

Pick the next table by what the enterprise admin backend actually reads
first (likely `providers` or `conversations`, given how central they are to
the domain crates' FKs), port it the same way `users` was ported here
(read every migration touching it, write one fresh `CREATE TABLE` for
Postgres, verify against a real instance), and only after a handful of
core tables are covered, tackle the domain-crate migration-runner
abstraction — that unblocks all 7 domain crates at once rather than
one at a time.
