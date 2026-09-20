//! `dreamcore session` — read-only session listing for @@ delivery.

use std::io::{self, Write};
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::cli::{SessionArgs, SessionCommand};
use crate::commands::session_capabilities;

const ENV_BASE_URL: &str = "ONE_BASE_URL";
const ENV_USER_ID: &str = "ONE_USER_ID";
const ENV_CONVERSATION_ID: &str = "ONE_CONVERSATION_ID";
const ENV_RUNTIME_TOKEN: &str = "ONE_RUNTIME_TOKEN";

pub(crate) async fn run_session(args: SessionArgs) -> ExitCode {
    match run_inner(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

async fn run_inner(args: SessionArgs) -> Result<(), ExitCode> {
    match args.command {
        SessionCommand::Capabilities => print_json(json!({
            "success": true,
            "data": session_capabilities::data(),
            "meta": { "schema_version": 1 }
        })),
        SessionCommand::List => run_list().await,
        SessionCommand::Unknown(path) => {
            let sub = path
                .iter()
                .map(|p| p.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            eprintln!("SESSION_CLI_UNKNOWN_COMMAND command=\"session\": unknown subcommand {sub}");
            Err(ExitCode::from(2))
        }
    }
}

async fn run_list() -> Result<(), ExitCode> {
    let env = runtime_env("session list")?;
    let url = format!(
        "{}/api/conversations?limit=100",
        env.base_url.trim_end_matches('/')
    );
    let mut request = reqwest::Client::new()
        .get(&url)
        .header("content-type", "application/json")
        .header("x-dream-user-id", &env.user_id)
        .header("x-dream-conversation-id", &env.conversation_id);
    if let Some(token) = &env.runtime_token {
        request = request.header("x-dream-runtime-token", token);
    } else {
        eprintln!("SESSION_CLI_AUTH_FAILED command=\"session list\": missing ONE_RUNTIME_TOKEN");
        return Err(ExitCode::from(1));
    }
    let response = request.send().await.map_err(|e| {
        eprintln!("SESSION_CLI_HTTP_FAILED command=\"session list\": {e}");
        ExitCode::from(1)
    })?;
    if !response.status().is_success() {
        eprintln!(
            "SESSION_CLI_HTTP_FAILED command=\"session list\": HTTP {}",
            response.status()
        );
        return Err(ExitCode::from(1));
    }
    let body: Value = response.json().await.map_err(|e| {
        eprintln!("SESSION_CLI_PARSE_FAILED command=\"session list\": {e}");
        ExitCode::from(1)
    })?;
    let rows = body
        .get("data")
        .and_then(|d| d.get("conversations"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    print_json(json!({
        "success": true,
        "data": { "conversations": rows },
        "meta": { "schema_version": 1, "command": "session list" }
    }))
}

struct SessionEnv {
    base_url: String,
    user_id: String,
    conversation_id: String,
    runtime_token: Option<String>,
}

fn runtime_env(command: &str) -> Result<SessionEnv, ExitCode> {
    fn req(name: &str, command: &str) -> Result<String, ExitCode> {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                eprintln!("SESSION_CLI_ENV_MISSING command=\"{command}\": missing {name}");
                ExitCode::from(1)
            })
    }
    Ok(SessionEnv {
        base_url: req(ENV_BASE_URL, command)?,
        user_id: req(ENV_USER_ID, command)?,
        conversation_id: req(ENV_CONVERSATION_ID, command)?,
        runtime_token: std::env::var(ENV_RUNTIME_TOKEN)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty()),
    })
}

fn print_json(value: Value) -> Result<(), ExitCode> {
    let rendered = serde_json::to_string_pretty(&value).map_err(|_| ExitCode::from(1))?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(rendered.as_bytes())
        .and_then(|_| stdout.write_all(b"\n"))
        .map_err(|_| ExitCode::from(1))
}
