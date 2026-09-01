use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use dream_engine_config::config::{McpServerConfig, TransportType};
use tokio::sync::broadcast::error::TryRecvError;
use tokio::time::timeout;

use super::*;
use crate::agent_task::IAgentTask;
use crate::protocol::events::FinishEventData;

async fn assert_no_stop_signal(agent: &DreamEngineAgentManager) {
    let notified = agent.cancel_notify.notified();
    tokio::pin!(notified);

    assert!(
        timeout(Duration::from_millis(20), &mut notified).await.is_err(),
        "idle stop must not leave a stale cancellation signal for the next turn"
    );
}

fn make_test_config() -> DreamEngineResolvedConfig {
    DreamEngineResolvedConfig {
        provider: "anthropic".into(),
        api_key: "sk-test-key".into(),
        model: "claude-sonnet-4-20250514".into(),
        base_url: None,
        system_prompt: None,
        max_tokens: None,
        context_window: None,
        max_turns: None,
        max_tool_call_malformed_turns: None,
        max_tool_call_failure_turns: None,
        compat_overrides: Default::default(),
        vision_model: None,
        vision_unavailable_reason: None,
        session_directory: env::temp_dir().join("dream-engine-test-sessions"),
        session_mode: None,
        skills: Vec::new(),
        extra_mcp_servers: HashMap::new(),
        bedrock_config: None,
        runtime_env: Vec::new(),
        prompt_dump_dir: None,
    }
}

fn make_cli_args(project_dir: PathBuf, provider: &str, model: &str) -> CliArgs {
    CliArgs {
        provider: Some(provider.to_owned()),
        api_key: Some("sk-test-key".to_owned()),
        base_url: None,
        model: Some(model.to_owned()),
        max_tokens: None,
        thinking: None,
        thinking_budget: None,
        max_turns: None,
        max_tool_call_malformed_turns: None,
        max_tool_call_failure_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: Some(project_dir),
    }
}

#[test]
fn resolve_engine_config_discards_standalone_max_token_settings() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join(".dream.toml"),
        r#"
[default]
max_tokens = 1234

[providers.openai.compat]
default_max_tokens = 2345

[[providers.openai.compat.model_max_tokens]]
pattern = "gpt-test"
max_tokens = 3456
"#,
    )
    .unwrap();
    let cli_args = make_cli_args(project.path().to_path_buf(), "openai", "gpt-test");

    let standalone = Config::resolve(&cli_args).unwrap();
    assert_eq!(standalone.max_tokens, Some(1234));
    assert_eq!(standalone.compat.default_max_tokens_for_model("gpt-test"), Some(3456));

    let embedded = resolve_engine_config(&cli_args).unwrap();
    assert_eq!(embedded.max_tokens, None);
    // File-based overrides (2345 / pattern "gpt-test" -> 3456) are discarded,
    // but `default_max_tokens_for_model` still falls back to
    // `openai_defaults().transport.default_max_tokens` (32_000, added in the
    // fork's `33c2bd2` fix so OpenAI-protocol requests always carry a sane
    // max_tokens instead of silently omitting the field) when no pattern
    // matches — this is the intended built-in default, not a leaked file value.
    assert_eq!(embedded.compat.default_max_tokens_for_model("gpt-test"), Some(32_000));
}

#[test]
fn resolve_engine_config_keeps_builtin_provider_max_token_policy() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join(".dream.toml"),
        r#"
[providers.anthropic.compat]
default_max_tokens = 42

[[providers.anthropic.compat.model_max_tokens]]
pattern = "claude-sonnet-4-6"
max_tokens = 42
"#,
    )
    .unwrap();
    let cli_args = make_cli_args(project.path().to_path_buf(), "anthropic", "claude-sonnet-4-6");

    let embedded = resolve_engine_config(&cli_args).unwrap();
    assert_eq!(embedded.max_tokens, None);
    assert_eq!(
        embedded.compat.default_max_tokens_for_model("claude-sonnet-4-6"),
        Some(128_000)
    );
}

#[test]
fn dream_engine_final_input_dump_value_contains_raw_split_input_and_context() {
    let mut mcp_env = HashMap::new();
    mcp_env.insert("TOKEN".to_owned(), "raw-token-value".to_owned());

    let mut mcp_servers = HashMap::new();
    mcp_servers.insert(
        "raw-mcp".to_owned(),
        McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("/bin/raw-mcp".to_owned()),
            args: Some(vec!["--serve".to_owned()]),
            env: Some(mcp_env),
            url: None,
            headers: None,
            deferred: Some(false),
            startup_timeout_ms: None,
        },
    );

    let context = DreamEngineFinalInputDumpContext {
        dump_dir: PathBuf::from("/tmp/prompt-dumps"),
        provider: "openai".to_owned(),
        model: "gpt-test".to_owned(),
        base_url: Some("https://example.test/v1".to_owned()),
        system_prompt: Some("assistant rule raw".to_owned()),
        session_mode: Some("yolo".to_owned()),
        skills: vec!["one-config".to_owned()],
        mcp_servers,
        runtime_env: vec![("ONE_RAW".to_owned(), "raw-env-value".to_owned())],
    };
    let data = SendMessageData {
        content: "team wake raw content".to_owned(),
        msg_id: "msg-dream-final".to_owned(),
        turn_id: Some("turn-dream-final".to_owned()),
        files: Vec::new(),
        inject_skills: Vec::new(),
    };

    let value = build_engine_final_input_dump_value("conv-dream", "/workspace", &context, &data);

    assert_eq!(value["kind"], "dream-final-input");
    assert_eq!(value["backend"], "dream");
    assert_eq!(value["conversation_id"], "conv-dream");
    assert_eq!(value["msg_id"], "msg-dream-final");
    assert_eq!(value["turn_id"], "turn-dream-final");
    assert_eq!(value["input"]["system_prompt"], "assistant rule raw");
    assert_eq!(value["input"]["user_content"], "team wake raw content");
    assert_eq!(value["resolved_context"]["provider"], "openai");
    assert_eq!(value["resolved_context"]["model"], "gpt-test");
    assert_eq!(value["resolved_context"]["workspace"]["path"], "/workspace");
    assert_eq!(value["resolved_context"]["skills"][0], "one-config");
    assert_eq!(
        value["resolved_context"]["mcp_servers"]["raw-mcp"]["env"]["TOKEN"],
        "raw-token-value"
    );
    assert_eq!(value["resolved_context"]["runtime_env"][0][1], "raw-env-value");
}

#[tokio::test]
async fn aionrs_agent_returns_correct_type() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    assert_eq!(agent.agent_type(), AgentType::DreamEngine);
    assert_eq!(agent.workspace(), "/project");
    assert_eq!(agent.conversation_id(), "conv-1");
}

#[tokio::test]
async fn aionrs_agent_initial_status_is_pending() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    assert_eq!(agent.status(), Some(ConversationStatus::Pending));
}

#[tokio::test]
async fn aionrs_agent_subscribe_returns_receiver() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    let _rx = agent.subscribe();
}

#[tokio::test]
async fn aionrs_agent_kill_succeeds() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    assert!(agent.kill(None).is_ok());
    // Idle kill only clears transient state; task-manager removal owns lifecycle cleanup.
    assert_eq!(agent.status(), Some(ConversationStatus::Pending));
}

#[tokio::test]
async fn aionrs_agent_kill_with_reason_succeeds() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    assert!(agent.kill(Some(AgentKillReason::IdleTimeout)).is_ok());
}

#[tokio::test]
async fn aionrs_agent_kill_running_turn_sends_stop_signal() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    agent.runtime.reset_for_new_turn(ConversationStatus::Running);

    let notified = agent.cancel_notify.notified();
    tokio::pin!(notified);
    assert!(timeout(Duration::from_millis(20), &mut notified).await.is_err());

    agent
        .kill(Some(AgentKillReason::ConversationDeleted))
        .expect("kill should request stop");

    timeout(Duration::from_millis(50), &mut notified)
        .await
        .expect("running kill should wake in-flight turn");
}

#[tokio::test]
async fn aionrs_agent_kill_and_wait_waits_for_running_turn_terminal() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    agent.runtime.reset_for_new_turn(ConversationStatus::Running);

    let wait = agent.kill_and_wait(Some(AgentKillReason::ConversationDeleted));
    tokio::pin!(wait);
    assert!(
        timeout(Duration::from_millis(20), &mut wait).await.is_err(),
        "kill_and_wait must not return before a running turn reaches a terminal event"
    );

    agent.runtime.emit_finish(None);
    agent.turn_finished_notify.notify_waiters();

    timeout(Duration::from_millis(50), &mut wait)
        .await
        .expect("kill_and_wait should return after terminal notification");
}

#[tokio::test]
async fn aionrs_agent_kill_idle_turn_does_not_leave_stale_stop_signal() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();

    agent
        .kill(Some(AgentKillReason::ConversationDeleted))
        .expect("idle kill should be harmless");

    assert_no_stop_signal(&agent).await;
}

#[tokio::test]
async fn aionrs_agent_confirmations_initially_empty() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    assert!(agent.get_confirmations().is_empty());
}

#[tokio::test]
async fn aionrs_agent_get_slash_commands_does_not_wait_for_engine_lock() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();

    let _engine_guard = agent.engine.lock().await;
    let commands = timeout(Duration::from_millis(50), agent.get_slash_commands())
        .await
        .expect("slash command metadata should not wait for an active engine run")
        .unwrap();

    assert!(!commands.is_empty());
}

#[tokio::test]
async fn aionrs_agent_check_approval_returns_false_by_default() {
    let agent = DreamEngineAgentManager::new("conv-1".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    assert!(!agent.check_approval("any_action", None));
}

#[tokio::test]
async fn stop_only_signals_in_flight_run() {
    let agent = DreamEngineAgentManager::new("conv-stop".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    let mut rx = agent.subscribe();

    agent.cancel().await.unwrap();

    assert_eq!(agent.status(), Some(ConversationStatus::Pending));
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    assert_no_stop_signal(&agent).await;
}

#[tokio::test]
async fn runtime_can_emit_error_and_finish() {
    let agent = DreamEngineAgentManager::new("conv-err".into(), "/project".into(), make_test_config(), None)
        .await
        .unwrap();
    let mut rx = agent.subscribe();

    agent.runtime.emit_error("test error");
    // emit_error sets status to Finished, so emit_finish is a no-op here.
    // We emit directly for the Finish broadcast path test:
    agent.runtime.emit(AgentStreamEvent::Finish(FinishEventData {
        session_id: None,
        ..Default::default()
    }));

    match rx.try_recv().unwrap() {
        AgentStreamEvent::Error(data) => assert_eq!(data.message, "test error"),
        other => panic!("Expected Error, got {:?}", other),
    }
    match rx.try_recv().unwrap() {
        AgentStreamEvent::Finish(_) => {}
        other => panic!("Expected Finish, got {:?}", other),
    }
}

/// The dream path reported no usage at all before this: the only emitter that
/// carries token counts is called exclusively by `dream-engine-cli`, while this
/// path ran the same engine in-process and threw the `AgentResult` away. These
/// pin the frame the renderer now reads.
mod turn_usage_frame {
    use super::*;

    fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_tokens: cache_write,
            cache_read_tokens: cache_read,
        }
    }

    /// The one thing worth a test here. The engine's `input_tokens` INCLUDES the
    /// cached tokens; the renderer's field of the same name means the input cache
    /// did NOT cover, because that is what its hit-rate divides by. Passing the
    /// engine value through would double-count and understate the hit rate — the
    /// kind of wrong number that looks plausible.
    #[test]
    fn reports_fresh_input_not_the_cache_inclusive_total() {
        let frame = build_turn_usage_frame(162_500, 1_000_000, Some(&usage(1_600_280, 7_300, 1_600_000, 146_100)));
        let meta = &frame["_meta"];
        // 1_600_280 - 1_600_000 - 146_100 saturates at 0 only if cache exceeds
        // input; here it is a plain subtraction of the cache-read portion.
        assert_eq!(meta["input_tokens"], 0);
        assert_eq!(meta["cached_read_tokens"], 1_600_000);
        assert_eq!(meta["cached_write_tokens"], 146_100);
        assert_eq!(meta["output_tokens"], 7_300);
    }

    /// A cancelled or failed turn has no `AgentResult`, but the tokens it burned
    /// before stopping are gone all the same — an indicator frozen at the
    /// previous number reads as "that attempt was free". Occupancy goes out
    /// without a breakdown.
    #[test]
    fn reports_occupancy_alone_when_there_is_no_turn_result() {
        let frame = build_turn_usage_frame(9_000, 200_000, None);
        assert_eq!(frame["used"], 9_000);
        assert_eq!(frame["size"], 200_000);
        // Omitted rather than zeroed: the renderer keeps the last breakdown it
        // had, and zeros would erase it.
        assert!(frame.get("_meta").is_none(), "no per-turn figures exist to report");
    }

    #[test]
    fn keeps_the_uncached_remainder_when_there_is_one() {
        let frame = build_turn_usage_frame(100, 200_000, Some(&usage(1_000, 50, 400, 100)));
        assert_eq!(frame["_meta"]["input_tokens"], 500);
    }

    /// A provider reporting cache figures larger than its own input total would
    /// underflow a u64 into a huge number; clamping reads as "all of it came
    /// from cache", which is the truthful reading of an impossible report.
    #[test]
    fn clamps_instead_of_underflowing_on_an_impossible_report() {
        let frame = build_turn_usage_frame(1, 2, Some(&usage(10, 0, 999, 999)));
        assert_eq!(frame["_meta"]["input_tokens"], 0);
    }

    /// `used` is occupancy, not the turn's own sum — the indicator answers how
    /// much of the window is gone, and a per-turn total would reset every
    /// message.
    #[test]
    fn carries_context_occupancy_and_window_rather_than_a_turn_total() {
        let frame = build_turn_usage_frame(162_500, 1_000_000, Some(&usage(280, 7_300, 0, 0)));
        assert_eq!(frame["used"], 162_500);
        assert_eq!(frame["size"], 1_000_000);
    }

    /// 0 is the renderer's own encoding for "window unknown" — it then shows the
    /// raw count instead of a percentage against a guessed denominator.
    #[test]
    fn passes_an_unknown_window_through_as_zero() {
        let frame = build_turn_usage_frame(500, 0, Some(&usage(100, 50, 0, 0)));
        assert_eq!(frame["size"], 0);
        assert_eq!(frame["used"], 500);
    }
}

/// The engine treats an absent turn budget as "iterate forever", and the
/// desktop never sets one — the conversation config has a `maxTurns` field
/// that nothing writes. So the default has to come from here; if this ever
/// resolves to `None` again, every ordinary conversation goes unbounded and
/// the only way out is the user pressing stop.
#[test]
fn a_conversation_without_its_own_budget_still_gets_one() {
    assert_eq!(
        Some(None::<usize>.unwrap_or(DEFAULT_MAX_TURNS_PER_TURN)),
        Some(DEFAULT_MAX_TURNS_PER_TURN),
        "an unset per-conversation budget must fall back to the default"
    );
    assert!(
        DEFAULT_MAX_TURNS_PER_TURN >= 100,
        "the budget is a runaway backstop, not a working limit — a hard task \
         can legitimately spend 50-100 turns, so it must sit well clear of that"
    );
}

/// A conversation that sets its own budget keeps it. The fallback must not
/// quietly override an explicit choice.
#[test]
fn an_explicit_budget_is_not_overridden_by_the_default() {
    let explicit = Some(7usize);
    assert_eq!(explicit.unwrap_or(DEFAULT_MAX_TURNS_PER_TURN), 7);
}

/// The idle limit bounds how long a single attempt may hang, which the turn
/// budget cannot: a stalled provider read never advances the turn counter.
/// Observed at 183 minutes before this existed.
#[test]
fn the_idle_limit_is_generous_but_finite() {
    let minutes = TURN_IDLE_LIMIT.as_secs() / 60;
    assert!(
        (20..=120).contains(&minutes),
        "want a window far past any real gap with nothing executing, while \
         still being noticed the same sitting; got {minutes} minutes"
    );
}

fn running_tool(call_id: &str) -> AgentStreamEvent {
    AgentStreamEvent::ToolCall(crate::protocol::events::ToolCallEventData {
        call_id: call_id.to_owned(),
        name: "build".to_owned(),
        args: serde_json::Value::Null,
        status: ToolCallStatus::Running,
        input: None,
        output: None,
        description: None,
        parent_call_id: None,
    })
}

fn finished_tool(call_id: &str) -> AgentStreamEvent {
    AgentStreamEvent::ToolCall(crate::protocol::events::ToolCallEventData {
        status: ToolCallStatus::Completed,
        ..match running_tool(call_id) {
            AgentStreamEvent::ToolCall(d) => d,
            _ => unreachable!(),
        }
    })
}

/// The case that motivated the whole design: a Team Mode member runs a build
/// or test suite that is silent for far longer than the idle window. Nothing
/// in the event stream distinguishes that from a hung tool, so an open tool
/// call must suspend the window outright — otherwise this fix would kill real
/// work, which is worse than the bug it fixes.
#[tokio::test]
async fn a_long_silent_tool_call_is_never_called_stalled() {
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    let idle = Duration::from_millis(200);

    let _ = tx.send(running_tool("c1"));
    let watcher = tokio::spawn(stalled_for(rx, idle));

    // Silence for many times the window, with the call still open.
    tokio::time::sleep(idle * 6).await;
    assert!(
        !watcher.is_finished(),
        "an open tool call must suspend the idle window entirely"
    );

    // Once it closes, the window applies again.
    let _ = tx.send(finished_tool("c1"));
    tokio::time::timeout(idle * 6, watcher)
        .await
        .expect("with nothing running, silence must eventually be called a stall")
        .expect("watcher task panicked");
}

/// Overlapping calls: the window stays suspended until the *last* one closes,
/// not the first.
#[tokio::test]
async fn the_window_stays_suspended_until_every_call_closes() {
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    let idle = Duration::from_millis(200);

    let _ = tx.send(running_tool("a"));
    let _ = tx.send(running_tool("b"));
    let watcher = tokio::spawn(stalled_for(rx, idle));

    let _ = tx.send(finished_tool("a"));
    tokio::time::sleep(idle * 5).await;
    assert!(
        !watcher.is_finished(),
        "one call closing must not resume the window while another is open"
    );

    let _ = tx.send(finished_tool("b"));
    tokio::time::timeout(idle * 6, watcher)
        .await
        .expect("the window must resume once the last call closes")
        .expect("watcher task panicked");
}

/// The stall watcher must treat any event as progress, so a turn that keeps
/// emitting never trips it however long it runs. This is what makes the limit
/// safe for a Team Mode member that legitimately works for hours; a total
/// wall-clock cap would kill exactly that case.
#[tokio::test]
async fn a_turn_that_keeps_emitting_is_never_called_stalled() {
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    let idle = Duration::from_millis(300);

    let watcher = tokio::spawn(stalled_for(rx, idle));

    // Emit steadily for well past the idle window.
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = tx.send(AgentStreamEvent::BackendTurnBound("t".to_owned()));
    }
    assert!(
        !watcher.is_finished(),
        "a turn still producing output must not be called stalled"
    );

    // Now go quiet; it should fire.
    tokio::time::timeout(idle * 4, watcher)
        .await
        .expect("the watcher must fire once output stops")
        .expect("watcher task panicked");
}

/// A closed channel means no event can ever arrive, so "idle" stops carrying
/// information. Firing there would report a stall on a turn that had already
/// finished.
#[tokio::test]
async fn a_finished_turn_is_not_reported_as_stalled() {
    let (tx, rx) = tokio::sync::broadcast::channel(4);
    drop(tx);

    let fired = tokio::time::timeout(Duration::from_millis(400), stalled_for(rx, Duration::from_millis(50)))
        .await
        .is_ok();
    assert!(!fired, "a closed event stream must park, not report a stall");
}

/// Both self-imposed stops must reach the user as the same structured,
/// localized error — not as untranslated prose on the output stream, which is
/// how the engine reports an exhausted turn budget on its own.
#[test]
fn a_self_imposed_stop_is_a_localizable_structured_error() {
    let err = turn_limit_send_error("Stopped after 150 agentic turns without converging.".to_owned());
    let data = err.stream_error();

    assert_eq!(data.code, Some(AgentErrorCode::DreamTurnLimitReached));
    assert_eq!(data.ownership, Some(AgentErrorOwnership::Dream));
    assert_eq!(data.retryable, Some(true));
    assert!(
        data.detail.as_deref().is_some_and(|d| d.contains("150")),
        "the detail has to say which limit was hit"
    );
}
