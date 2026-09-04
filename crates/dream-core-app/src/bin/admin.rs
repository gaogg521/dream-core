//! `dreamcore-admin`: standalone governance-plane server.
//!
//! Companion process to `dreamcore` (see `src/main.rs`). Shares the same
//! data directory and database as the main server — the two binaries come
//! from the same compilation, so their view of the schema can never drift
//! (see dream-en's docs/roadmap.zh-CN.md, E3 "为什么不迁仓"). This binary
//! never touches conversations, agents, files, MCP, channels, cron, or
//! WebSockets; it only mounts the governance plane built by
//! `dream_core_app::create_admin_router`.
//!
//! Deliberately does not replicate `dreamcore`'s desktop-oriented bootstrap
//! (single-instance flock, parent-process watchdog, `DREAMCORE_LISTENING` /
//! `DREAMCORE_READY` stdout markers, builtin-skill materialization): none of
//! that applies to a server-only governance process with no desktop parent.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use dream_core_app::{AppConfig, AppServices, IdentityMode, create_admin_router};

#[derive(Parser)]
#[command(
    name = "dreamcore-admin",
    about = "One Work governance-plane backend server",
    version
)]
struct Cli {
    /// Host address to listen on. Also settable via `ONE_HOST`.
    #[arg(long, env = "ONE_HOST", default_value_t = String::from(dream_core_common::constants::DEFAULT_HOST))]
    host: String,

    /// Port number to listen on. Also settable via `ONE_ADMIN_PORT`.
    ///
    /// Defaults to `dreamcore`'s own default port + 1 so the two can run
    /// side by side without a config change.
    #[arg(long, env = "ONE_ADMIN_PORT", default_value_t = dream_core_common::constants::DEFAULT_PORT + 1)]
    port: u16,

    /// Data directory for the shared database. MUST point at the same
    /// directory as the `dreamcore` process it is paired with. Also settable
    /// via `ONE_DATA_DIR`.
    #[arg(long, env = "ONE_DATA_DIR", default_value = "data")]
    data_dir: PathBuf,

    /// Log level filter (e.g. "info", "debug"). Also settable via
    /// `ONE_LOG_LEVEL`.
    #[arg(long, env = "ONE_LOG_LEVEL")]
    log_level: Option<String>,
}

fn main() -> ExitCode {
    // Same contract as the main binary: adopt legacy env names before `clap`
    // reads them, while this process is still single-threaded.
    unsafe { dream_core_common::adopt_legacy_env() };

    let cli = Cli::parse();
    init_tracing(cli.log_level.as_deref());

    dream_core_runtime::init(&cli.data_dir);

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("ADMIN_RUNTIME_INIT_FAILED {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(async_main(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ADMIN_STARTUP_FAILED {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(log_level: Option<&str>) {
    use tracing_subscriber::EnvFilter;

    let filter = match log_level {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn async_main(cli: Cli) -> anyhow::Result<()> {
    let config = AppConfig {
        host: cli.host,
        port: cli.port,
        data_dir: cli.data_dir.clone(),
        // No conversation workspaces ever open in this process; the field
        // still has to be set to something for `AppServices::from_config`.
        work_dir: cli.data_dir.clone(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        local: false,
        identity_mode: IdentityMode::WebUi,
        bootstrap_secret: None,
        dump_prompts: false,
        recover_corrupted_database: false,
    };

    let db_path = config.database_path();
    tracing::info!(path = %db_path.display(), "startup: opening shared database");
    let database = dream_core_db::init_database_staged_with_options(
        &db_path,
        dream_core_db::DatabaseInitOptions {
            recover_corrupted_database: false,
        },
    )
    .await?;

    let services = AppServices::from_config(database, &config).await?;
    let router = create_admin_router(&services).await?;

    let addr = config.socket_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(address = %addr, "dreamcore-admin: listening");

    // See the matching call in `commands/cmd_server.rs`: the admin plane is
    // exactly where the IP allowlist matters most, and a bare `Router` never
    // injects the `ConnectInfo` that `resolve_caller_ip` needs to answer with
    // anything but `None`.
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    tracing::info!("dreamcore-admin: shutting down");
    services.database.close().await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
