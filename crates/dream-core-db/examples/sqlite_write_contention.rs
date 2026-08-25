//! Measures what happens when two processes write the same SQLite file.
//!
//! # Why this exists
//!
//! The enterprise split (dream-en `docs/roadmap.zh-CN.md`, E3) puts a second
//! process — the admin service — on the same database as `dreamcore`. SQLite
//! allows one writer at a time, so the shared-database route is only viable if
//! an admin-side bulk write cannot stall the conversation path. This harness is
//! what that decision rests on. It is an investigation tool, not a regression
//! test, which is why it is an example rather than a `tests/` file: CI should
//! not spend a minute on it every run.
//!
//! # Why it spawns real processes
//!
//! Two pools inside one process would be a weaker experiment. SQLite arbitrates
//! writers through file locks, and on POSIX those are owned by the *process* —
//! so a same-process test can both invent contention that production would not
//! see and miss failure modes that it would. The coordinator therefore
//! re-executes this binary twice and lets the OS arbitrate, as it will in the
//! real deployment.
//!
//! # Running
//!
//! ```text
//! cargo run -p dream-core-db --example sqlite_write_contention
//! ```
//!
//! Environment overrides:
//!   `CONTENTION_SECS`   how long both roles run           (default 20)
//!   `CONTENTION_BULK`   rows per admin bulk transaction   (default 2000)
//!   `CONTENTION_GAP_MS` pause between admin transactions  (default 1000)

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sqlx::pool::PoolOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Row, Sqlite, SqlitePool};

/// Mirrors `dream_core_db::database`'s production settings. Diverging here
/// would make the measurement describe something other than what we ship.
const BUSY_TIMEOUT_MS: u64 = 5000;
const MAX_CONNECTIONS: u32 = 5;

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

async fn open(path: &Path) -> SqlitePool {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .expect("connect options")
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))
        .journal_mode(SqliteJournalMode::Wal);
    PoolOptions::<Sqlite>::new()
        .max_connections(MAX_CONNECTIONS)
        .connect_with(opts)
        .await
        .expect("open pool")
}

/// Stands in for the conversation path: many small, latency-sensitive writes.
async fn role_chat(path: &Path) {
    let pool = open(path).await;
    let deadline = Instant::now() + Duration::from_secs(env_u64("CONTENTION_SECS", 20));

    let mut latencies_us: Vec<u128> = Vec::new();
    let mut failures = 0u64;
    let mut first_failures: Vec<String> = Vec::new();

    while Instant::now() < deadline {
        let started = Instant::now();
        let result = sqlx::query("INSERT INTO chat_writes (payload, at_ms) VALUES (?, ?)")
            .bind("turn")
            .bind(now_ms())
            .execute(&pool)
            .await;
        match result {
            Ok(_) => latencies_us.push(started.elapsed().as_micros()),
            Err(e) => {
                failures += 1;
                if first_failures.len() < 5 {
                    first_failures.push(e.to_string());
                }
            }
        }
        // ~50 writes/sec, roughly the rate a busy conversation turn produces.
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    latencies_us.sort_unstable();
    let pct = |p: usize| -> f64 {
        if latencies_us.is_empty() {
            0.0
        } else {
            latencies_us[(latencies_us.len() - 1) * p / 100] as f64 / 1000.0
        }
    };

    println!("CHAT writes={} failures={}", latencies_us.len(), failures);
    println!(
        "CHAT latency_ms p50={:.1} p95={:.1} p99={:.1} max={:.1}",
        pct(50),
        pct(95),
        pct(99),
        latencies_us.last().copied().unwrap_or(0) as f64 / 1000.0
    );
    for failure in first_failures {
        println!("CHAT failure: {failure}");
    }
}

/// Stands in for the admin path: a bulk write in one transaction, the shape a
/// directory sync or a bulk invite takes.
async fn role_admin(path: &Path) {
    let pool = open(path).await;
    let rows = env_u64("CONTENTION_BULK", 2000);
    let gap = env_u64("CONTENTION_GAP_MS", 1000);
    let deadline = Instant::now() + Duration::from_secs(env_u64("CONTENTION_SECS", 20));

    let mut batches = 0u64;
    let mut failures = 0u64;
    let mut worst_ms = 0u128;

    while Instant::now() < deadline {
        let started = Instant::now();
        let outcome: Result<(), sqlx::Error> = async {
            let mut tx = pool.begin().await?;
            for i in 0..rows {
                sqlx::query("INSERT INTO admin_writes (batch, idx, at_ms) VALUES (?, ?, ?)")
                    .bind(batches as i64)
                    .bind(i as i64)
                    .bind(now_ms())
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            Ok(())
        }
        .await;

        let elapsed = started.elapsed().as_millis();
        match outcome {
            Ok(()) => {
                batches += 1;
                worst_ms = worst_ms.max(elapsed);
                println!("ADMIN batch rows={rows} took_ms={elapsed}");
            }
            Err(e) => {
                failures += 1;
                println!("ADMIN batch FAILED after_ms={elapsed}: {e}");
            }
        }
        tokio::time::sleep(Duration::from_millis(gap)).await;
    }

    println!("ADMIN batches={batches} failures={failures} worst_ms={worst_ms}");
}

async fn coordinate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db: PathBuf = dir.path().join("contention.db");

    // Create the schema (and the WAL) before either child opens the file, so
    // the measurement covers writing rather than first-open setup.
    {
        let pool = open(&db).await;
        sqlx::query("CREATE TABLE chat_writes (id INTEGER PRIMARY KEY, payload TEXT, at_ms INTEGER)")
            .execute(&pool)
            .await
            .expect("create chat_writes");
        sqlx::query("CREATE TABLE admin_writes (id INTEGER PRIMARY KEY, batch INTEGER, idx INTEGER, at_ms INTEGER)")
            .execute(&pool)
            .await
            .expect("create admin_writes");
        let mode: String = sqlx::query("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .expect("journal_mode")
            .get(0);
        println!("journal_mode={mode} busy_timeout_ms={BUSY_TIMEOUT_MS} max_connections={MAX_CONNECTIONS}/process");
        pool.close().await;
    }

    let exe = std::env::current_exe().expect("current exe");
    let mut children: Vec<std::process::Child> = ["chat", "admin"]
        .iter()
        .map(|role| {
            std::process::Command::new(&exe)
                .arg(role)
                .arg(&db)
                .spawn()
                .expect("spawn child")
        })
        .collect();

    for child in &mut children {
        let status = child.wait().expect("child wait");
        if !status.success() {
            println!("child exited with {status}");
        }
    }

    println!();
    println!("--- how to read this ---");
    println!("CHAT failures must be 0. Anything above that means the 5s busy_timeout");
    println!("does not absorb an admin bulk write, and the shared-database route fails.");
    println!("Compare CHAT p99 against ADMIN took_ms: a p99 approaching the batch");
    println!("duration is the conversation path being held behind the write transaction.");
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("chat"), Some(path)) => role_chat(Path::new(&path)).await,
        (Some("admin"), Some(path)) => role_admin(Path::new(&path)).await,
        (None, _) => coordinate().await,
        (Some(other), _) => {
            eprintln!("unknown role {other:?}; run with no arguments to coordinate");
            std::process::exit(2);
        }
    }
}
