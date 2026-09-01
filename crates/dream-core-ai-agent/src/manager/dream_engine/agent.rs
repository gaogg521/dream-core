use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use dream_core_api_types::{
    AcpConfigOptionDto, AcpConfigSelectOptionDto, AgentErrorCode, AgentErrorOwnership, AgentErrorResolution,
    AgentErrorResolutionKind, AgentModeResponse, ConfigOptionConfirmation, GetConfigOptionsResponse,
    SetConfigOptionResponse, SlashCommandItem,
};
use dream_core_common::{
    AgentKillReason, AgentType, Confirmation, ConversationStatus, ErrorChain, TimestampMs, generate_short_id, now_ms,
};
use dream_engine_agent::bootstrap::AgentBootstrap;
use dream_engine_agent::engine::{AgentEngine, AgentResult};
use dream_engine_agent::output::OutputSink;
use dream_engine_agent::session::Session;
use dream_engine_config::compat::ProviderCompat;
use dream_engine_config::config::{CliArgs, Config, McpServerConfig, ProviderType};
use dream_engine_mcp::manager::McpManager;
use dream_engine_protocol::commands::{ApprovalScope, SessionMode};
use dream_engine_protocol::{ToolApprovalManager, ToolApprovalResult};
use dream_engine_types::message::{StopReason, TokenUsage};
use serde_json::Value;
use tokio::sync::{Mutex, Notify, broadcast};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use crate::agent_runtime::AgentRuntime;
use crate::agent_task::IAgentTask;
use crate::capability::backend_output_sink::BackendOutputSink;
use crate::capability::backend_protocol_sink::BackendProtocolSink;
use crate::capability::image_input::resolve_image_input_capability;
use crate::dev_prompt_dump::{AgentFinalInputDump, dump_agent_final_input};
use crate::error::AgentError;
use crate::protocol::events::{AgentStreamEvent, ToolCallStatus};
use crate::protocol::send_error::AgentSendError;
use crate::types::{DreamEngineResolvedConfig, SendMessageData};

use super::content::build_content_blocks;
use super::error::{engine_error_to_send_error, engine_runtime_error_summary};

/// Ceiling on agentic turns within one user message, when the conversation
/// does not set its own.
///
/// A backstop, not a working limit: a genuinely hard task can legitimately
/// spend 50-100 turns, so this sits well clear of normal work and only catches
/// a run that is never going to converge. Without it the loop is unbounded —
/// one report had a single turn still running after 183 minutes, producing
/// nothing and spending tokens the whole time, because the only way out was
/// the user pressing stop.
const DEFAULT_MAX_TURNS_PER_TURN: usize = 150;

/// How long a turn may emit nothing, *with no tool running*, before we call it
/// stalled.
///
/// Deliberately an idle limit and not a total one. The failure being fixed is
/// a turn that is not doing anything, and a total cap cannot tell that apart
/// from a turn that is simply long: Team Mode members routinely work well past
/// any wall-clock number worth setting, and killing one mid-flight for being
/// productive too long would be a worse bug than the one being fixed.
///
/// The clock is also suspended entirely while a tool call is in flight — see
/// [`stalled_for`]. A build or test run can legitimately be silent for hours,
/// and from the event stream that is indistinguishable from a tool that has
/// hung, so the benefit of the doubt goes to the long one. What is left for
/// this window to catch is silence with *nothing executing*: a provider that
/// accepted the request and stopped answering, or an engine that lost its way
/// between steps. Thirty minutes is far past any real gap of that kind, while
/// still being noticed in the same sitting rather than after three hours.
const TURN_IDLE_LIMIT: Duration = Duration::from_secs(30 * 60);

/// How one turn ended. Named rather than `Option<Result<_>>` because there are
/// now three outcomes, and "timed out" must not be mistaken for "cancelled" —
/// a cancel is the user's own doing and finishes quietly, a timeout is ours
/// and has to say so.
enum TurnOutcome<T> {
    Completed(T),
    Cancelled,
    TimedOut,
}

/// Resolves once the turn has emitted nothing for `idle` *and* no tool call is
/// in flight.
///
/// Watches the same event stream the UI renders, so "made progress" means
/// exactly what the user sees: any text, tool call, or thinking frame resets
/// the clock.
///
/// Tool calls additionally suspend it. A `ToolCall` frame with status
/// `Running` opens a call and a terminal status closes it; while any call is
/// open the window cannot expire. This is what makes the limit safe for the
/// case that motivated it — a team member running a long build or test suite
/// is silent for as long as that takes, and nothing in the stream
/// distinguishes that from a tool that has hung. The trade-off is explicit: a
/// genuinely hung *tool* is not caught here and the user must stop it. Silence
/// with nothing executing — a provider that stopped answering, an engine lost
/// between steps — is what this catches, and for that no legitimate gap comes
/// anywhere near the window.
async fn stalled_for(mut events: broadcast::Receiver<AgentStreamEvent>, idle: Duration) {
    let mut running_calls: std::collections::HashSet<String> = std::collections::HashSet::new();

    loop {
        // With a tool executing there is no deadline at all; just wait for the
        // next frame so the in-flight set stays current.
        let next = if running_calls.is_empty() {
            match tokio::time::timeout(idle, events.recv()).await {
                Err(_elapsed) => return, // silent for the whole window, nothing running
                Ok(next) => next,
            }
        } else {
            events.recv().await
        };

        match next {
            Ok(AgentStreamEvent::ToolCall(data)) => match data.status {
                ToolCallStatus::Running => {
                    running_calls.insert(data.call_id);
                }
                ToolCallStatus::Completed | ToolCallStatus::Error | ToolCallStatus::Canceled => {
                    running_calls.remove(&data.call_id);
                }
            },
            // Any other event is progress; the timeout above already reset.
            Ok(_) => {}
            // Events arrived faster than this watcher drained them —
            // emphatically progress, but the in-flight set is now unreliable,
            // so clear it rather than hold the window open on a call whose
            // terminal frame may have been the one dropped.
            Err(broadcast::error::RecvError::Lagged(_)) => running_calls.clear(),
            // The sender is gone, so no event can ever arrive and "idle" stops
            // meaning anything. Park instead of firing: the run future is
            // about to resolve on its own, and reporting a stall on a turn
            // that already finished would be a lie.
            Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
        }
    }
}

/// The user-facing error for a turn we stopped ourselves. `detail` names which
/// limit was hit; the localized title and body come from the code.
fn turn_limit_send_error(detail: String) -> AgentSendError {
    AgentSendError::new(
        "The request was stopped because it was not converging",
        AgentErrorCode::DreamTurnLimitReached,
        AgentErrorOwnership::Dream,
        Some(detail),
        // Retryable: an unproductive loop often does not repeat, and the user
        // has nothing to fix on their side first.
        true,
        false,
        Some(AgentErrorResolution::new(AgentErrorResolutionKind::Retry, None)),
    )
}

fn resolve_engine_config(cli_args: &CliArgs) -> Result<Config, AgentError> {
    let mut config =
        Config::resolve(cli_args).map_err(|e| AgentError::internal(format!("Config resolve failed: {e}")))?;

    // Dream UI owns the embedded runtime policy. Standalone dream max-token
    // settings must not leak in from global or workspace config files.
    config.max_tokens = None;
    let default_transport = match config.provider {
        ProviderType::Anthropic | ProviderType::Vertex => ProviderCompat::anthropic_defaults().transport,
        ProviderType::OpenAI => ProviderCompat::openai_defaults().transport,
        ProviderType::Bedrock => ProviderCompat::bedrock_defaults().transport,
        ProviderType::Ollama => ProviderCompat::ollama_defaults().transport,
    };
    config.compat.transport.default_max_tokens = default_transport.default_max_tokens;
    config.compat.transport.model_max_tokens = default_transport.model_max_tokens;

    Ok(config)
}

#[derive(Clone, Debug)]
struct DreamEngineFinalInputDumpContext {
    dump_dir: PathBuf,
    provider: String,
    model: String,
    base_url: Option<String>,
    system_prompt: Option<String>,
    session_mode: Option<String>,
    skills: Vec<String>,
    mcp_servers: HashMap<String, McpServerConfig>,
    runtime_env: Vec<(String, String)>,
}

fn build_engine_final_input_dump_value(
    conversation_id: &str,
    workspace: &str,
    context: &DreamEngineFinalInputDumpContext,
    data: &SendMessageData,
) -> Value {
    serde_json::json!({
        "kind": "dream-final-input",
        "backend": "dream",
        "conversation_id": conversation_id,
        "session_id": "none",
        "msg_id": data.msg_id,
        "turn_id": data.turn_id.as_deref().unwrap_or("none"),
        "input": {
            "system_prompt": context.system_prompt.as_deref(),
            "user_content": &data.content,
        },
        "resolved_context": {
            "provider": &context.provider,
            "model": &context.model,
            "base_url": context.base_url.as_deref(),
            "workspace": {
                "path": workspace,
            },
            "session_mode": context.session_mode.as_deref(),
            "skills": &context.skills,
            "mcp_servers": serde_json::to_value(&context.mcp_servers).unwrap_or(Value::Null),
            "runtime_env": &context.runtime_env,
        },
    })
}

pub struct DreamEngineAgentManager {
    runtime: AgentRuntime,
    engine: Mutex<AgentEngine>,
    /// Static slash command metadata captured at bootstrap so UI lookups do
    /// not wait behind an active `engine.run()` turn.
    slash_commands: Vec<SlashCommandItem>,
    /// Holds `Arc<McpManager>` instances alive for the duration of this agent's
    /// lifetime. The managers are not accessed after construction — they exist
    /// solely so their underlying MCP connections outlive the engine's event
    /// loop. Rust drops them here, in field-declaration order, after `engine`
    /// and `runtime` are dropped. See the explicit `Drop` impl below.
    #[allow(dead_code)] // intentional: lifetime-extension only; see Drop impl
    mcp_managers: Vec<Arc<McpManager>>,
    approval_manager: Arc<ToolApprovalManager>,
    confirmations: Arc<RwLock<Vec<Confirmation>>>,
    final_input_dump: Option<DreamEngineFinalInputDumpContext>,
    /// Signalled by `cancel()` to abort an in-flight `engine.run()` via
    /// `tokio::select!` in `send_message()`.
    cancel_notify: Arc<Notify>,
    /// Signalled after an in-flight turn emits its terminal event.
    turn_finished_notify: Arc<Notify>,
}

impl Drop for DreamEngineAgentManager {
    fn drop(&mut self) {
        // McpManagers are held alive by the `mcp_managers` field specifically
        // so they outlive the agent's event loop. No explicit cleanup is needed
        // here — the Arc drop path releases each McpManager's underlying MCP
        // connection. This impl exists to document the intentional Drop-order
        // semantics rather than as a lint escape hatch.
    }
}

impl DreamEngineAgentManager {
    pub async fn new(
        conversation_id: String,
        workspace: String,
        config_extra: DreamEngineResolvedConfig,
        resume_session: Option<Session>,
    ) -> Result<Self, AgentError> {
        let runtime = AgentRuntime::new(conversation_id.clone(), workspace.clone(), 128);
        let sink: Arc<dyn OutputSink> = Arc::new(BackendOutputSink::new(
            runtime.event_sender(),
            config_extra.model.clone(),
        ));
        let runtime_env = config_extra.runtime_env.clone();
        let image_input_override = config_extra.compat_overrides.image_input;
        let image_input_capability = image_input_override.unwrap_or_else(|| {
            resolve_image_input_capability(
                &config_extra.provider,
                config_extra.base_url.as_deref(),
                &config_extra.model,
            )
        });
        info!(
            conversation_id = %conversation_id,
            provider = %config_extra.provider,
            model = %config_extra.model,
            image_input_capability = ?image_input_capability,
            image_input_source = if image_input_override.is_some() { "provider_settings" } else { "catalog" },
            "Resolved image input capability for DreamEngine model"
        );
        let final_input_dump = config_extra
            .prompt_dump_dir
            .clone()
            .map(|dump_dir| DreamEngineFinalInputDumpContext {
                dump_dir,
                provider: config_extra.provider.clone(),
                model: config_extra.model.clone(),
                base_url: config_extra.base_url.clone(),
                system_prompt: config_extra.system_prompt.clone(),
                session_mode: config_extra.session_mode.clone(),
                skills: config_extra.skills.clone(),
                mcp_servers: config_extra.extra_mcp_servers.clone(),
                runtime_env: config_extra.runtime_env.clone(),
            });

        let cli_args = CliArgs {
            provider: Some(config_extra.provider.clone()),
            api_key: Some(config_extra.api_key.clone()),
            base_url: config_extra.base_url.clone(),
            model: Some(config_extra.model.clone()),
            max_tokens: None,
            // Never `None` here. The engine treats an absent budget as
            // "iterate forever" (`TurnTracker::limit_reached` can only fire on
            // `Some`), and the desktop never sets one — the `maxTurns` field
            // exists in the conversation config type but nothing writes it — so
            // every ordinary conversation ran unbounded. The tool-loop breakers
            // do not cover this: all of them, the cycle detector included, key
            // off *failing* tool calls, so a model that keeps making
            // successful-but-unproductive calls trips none of them and burns
            // tokens until the user notices and presses stop.
            max_turns: Some(config_extra.max_turns.unwrap_or(DEFAULT_MAX_TURNS_PER_TURN)),
            max_tool_call_malformed_turns: config_extra.max_tool_call_malformed_turns,
            max_tool_call_failure_turns: config_extra.max_tool_call_failure_turns,
            system_prompt: config_extra.system_prompt.clone(),
            profile: None,
            auto_approve: config_extra.session_mode.as_deref() == Some("yolo"),
            thinking: None,
            thinking_budget: None,
            project_dir: Some(PathBuf::from(&workspace)),
        };

        let mut config = resolve_engine_config(&cli_args)?;

        // User-declared context window: compaction must trigger inside the
        // real window (a local Ollama model's 8k, not the 200k default), and
        // the Ollama native transport sends it as `options.num_ctx` so the
        // daemon actually allocates that window.
        if let Some(context_window) = config_extra.context_window {
            config.compact.context_window = context_window as usize;
            config.compat.transport.num_ctx = Some(context_window);
        }

        // Backend-specific overrides
        config.bedrock = config_extra.bedrock_config;
        config.session.enabled = true;
        config.session.directory = config_extra.session_directory.to_string_lossy().into_owned();
        config.compat.image_input = Some(image_input_capability);
        // Lets dream's `ReadImage` tool turn images into text for a model that
        // cannot see them. Resolved by the factory from the user's configured
        // providers; `None` here means the tool will report images as
        // unreadable rather than let the agent guess.
        config.vision = config_extra.vision_model;
        // Why there is no delegate, when the factory knows a reason more
        // specific than "none configured" — today: the company's model
        // allowlist excluded every vision-capable model this user has. Without
        // it `ReadImage` tells them to add a vision model in Settings, which
        // for a policy refusal is both wrong and impossible to act on.
        let vision_unavailable_reason = config_extra.vision_unavailable_reason;

        if let Some(mode) = config_extra.compat_overrides.openai_api_mode {
            config.compat.transport.openai_api_mode = Some(mode);
        }
        if let Some(field) = config_extra.compat_overrides.max_tokens_field {
            config.compat.transport.max_tokens_field = Some(field);
        }
        if let Some(path) = config_extra.compat_overrides.api_path {
            config.compat.transport.api_path = Some(path);
        }

        if !config_extra.extra_mcp_servers.is_empty() {
            config.mcp.servers.extend(config_extra.extra_mcp_servers.clone());
        }

        let is_resume = resume_session.is_some();
        let provider_label = config.provider_label.clone();

        let mut bootstrap = AgentBootstrap::new(config, &workspace, sink)
            .runtime_env(runtime_env)
            .vision_unavailable_reason(vision_unavailable_reason);
        if let Some(session) = resume_session {
            info!(
                conversation_id = %conversation_id,
                session_id = %session.id,
                message_count = session.messages.len(),
                "Resuming DreamEngine session"
            );
            bootstrap = bootstrap.resume(session);
        }

        let result = bootstrap
            .build()
            .await
            .map_err(|e| AgentError::internal(format!("Agent bootstrap failed: {e}")))?;

        let mut engine = result.engine;
        if !is_resume && let Err(e) = engine.init_session(&provider_label, &workspace, Some(&conversation_id)) {
            error!(
                conversation_id = %conversation_id,
                error = %ErrorChain(&*e),
                "Failed to init session, continuing without persistence"
            );
        }

        let approval_manager = Arc::new(ToolApprovalManager::new());

        if let Some(mode_str) = &config_extra.session_mode {
            let mode = parse_session_mode(mode_str);
            approval_manager.set_mode(mode);
            info!(
                conversation_id = %conversation_id,
                session_mode = mode_str,
                "DreamEngine initial session mode applied"
            );
        }

        let confirmations = Arc::new(RwLock::new(Vec::new()));
        let protocol_sink = BackendProtocolSink::new(runtime.event_sender(), confirmations.clone());
        engine.set_approval_manager(approval_manager.clone());
        engine.set_protocol_writer(Arc::new(protocol_sink));
        let slash_commands = engine
            .slash_command_list()
            .into_iter()
            .map(|(command, description)| SlashCommandItem {
                command,
                description,
                completion_behavior: None,
                empty_turn_tip_code: None,
                empty_turn_tip_params: None,
            })
            .collect();

        runtime.transition_to(ConversationStatus::Pending);

        Ok(Self {
            runtime,
            engine: Mutex::new(engine),
            slash_commands,
            mcp_managers: result.mcp_managers,
            approval_manager,
            confirmations,
            final_input_dump,
            cancel_notify: Arc::new(Notify::new()),
            turn_finished_notify: Arc::new(Notify::new()),
        })
    }

    fn request_stop(&self, reason: Option<AgentKillReason>, operation: &'static str) -> bool {
        let was_running = self.runtime.status() == Some(ConversationStatus::Running);

        if let Ok(mut confs) = self.confirmations.write() {
            confs.clear();
        }

        if was_running {
            self.cancel_notify.notify_waiters();
        }

        info!(
            conversation_id = %self.runtime.conversation_id(),
            ?reason,
            was_running,
            operation,
            "DreamEngine stop signal requested"
        );

        was_running
    }

    fn dump_engine_final_input(&self, data: &SendMessageData) {
        let Some(context) = self.final_input_dump.as_ref() else {
            return;
        };

        let value = build_engine_final_input_dump_value(
            self.runtime.conversation_id(),
            self.runtime.workspace(),
            context,
            data,
        );
        let input = value.get("input").cloned().unwrap_or(Value::Null);
        let resolved_context = value.get("resolved_context").cloned().unwrap_or(Value::Null);

        match dump_agent_final_input(
            &context.dump_dir,
            AgentFinalInputDump {
                kind: "dream-final-input",
                backend: "dream",
                conversation_id: self.runtime.conversation_id(),
                session_id: None,
                msg_id: Some(data.msg_id.as_str()),
                turn_id: data.turn_id.as_deref(),
                input,
                resolved_context,
            },
        ) {
            Ok(path) => {
                debug!(
                    conversation_id = %self.runtime.conversation_id(),
                    msg_id = %data.msg_id,
                    path = %path.display(),
                    "DEV agent final input dump written"
                );
            }
            Err(error) => {
                warn!(
                    conversation_id = %self.runtime.conversation_id(),
                    msg_id = %data.msg_id,
                    error = %error,
                    "DEV agent final input dump failed"
                );
            }
        }
    }
}

/// Build the usage frame for one finished dream turn. Pure (FCIS core).
///
/// ⚠️ **The two `input_tokens` mean opposite things.** The engine's
/// `TokenUsage::input_tokens` is the FULL provider input — cache reads and cache
/// writes included (see its doc comment). The renderer's breakdown field of the
/// same name means the input that cache did NOT cover, because that is what
/// makes its cache hit-rate (`cached_read / (cached_read + input)`) mean
/// anything. Passing the engine's number through unchanged double-counts the
/// cached tokens and reports a hit rate far below the truth, so the cached parts
/// are subtracted here — the single reason this function is not a one-line
/// `json!`.
///
/// `used` is context occupancy rather than the turn's own total: the indicator
/// answers "how much of the window is gone", and a per-turn sum would reset with
/// every message. `size` of 0 is the renderer's own encoding for "window
/// unknown", which is exactly what a configuration without one resolves to.
fn build_turn_usage_frame(context_usage: u64, context_window: u64, usage: Option<&TokenUsage>) -> Value {
    let Some(usage) = usage else {
        // No per-turn figures to report: a cancelled or failed turn produced no
        // `AgentResult`, and a between-turns snapshot has none either. Occupancy
        // still moved — the tokens were spent — so the headline is sent without
        // a breakdown rather than withheld entirely. `_meta` is omitted, not
        // zeroed: the renderer keeps the last breakdown it had, and zeros would
        // erase it.
        return serde_json::json!({
            "used": context_usage,
            "size": context_window,
        });
    };

    // Saturating: a provider reporting cache figures larger than its own input
    // total would otherwise underflow into a huge number. Clamping to zero reads
    // as "all of it came from cache", the truthful reading of that report.
    let fresh_input = usage
        .input_tokens
        .saturating_sub(usage.cache_read_tokens)
        .saturating_sub(usage.cache_creation_tokens);

    serde_json::json!({
        "used": context_usage,
        "size": context_window,
        "_meta": {
            "input_tokens": fresh_input,
            "output_tokens": usage.output_tokens,
            "cached_read_tokens": usage.cache_read_tokens,
            "cached_write_tokens": usage.cache_creation_tokens,
        },
    })
}

#[async_trait::async_trait]
impl IAgentTask for DreamEngineAgentManager {
    fn agent_type(&self) -> AgentType {
        AgentType::DreamEngine
    }

    fn conversation_id(&self) -> &str {
        self.runtime.conversation_id()
    }

    fn workspace(&self) -> &str {
        self.runtime.workspace()
    }

    fn status(&self) -> Option<ConversationStatus> {
        self.runtime.status()
    }

    fn last_activity_at(&self) -> TimestampMs {
        self.runtime.last_activity_at()
    }

    fn subscribe(&self) -> broadcast::Receiver<AgentStreamEvent> {
        self.runtime.subscribe()
    }

    async fn send_message(&self, data: SendMessageData) -> Result<(), AgentSendError> {
        let started_at = now_ms();
        info!(
            conversation_id = %self.runtime.conversation_id(),
            msg_id = %data.msg_id,
            turn_id = data.turn_id.as_deref().unwrap_or("none"),
            "DreamEngine send_message started"
        );
        self.runtime.bump_activity();
        self.runtime.reset_for_new_turn(ConversationStatus::Running);
        self.dump_engine_final_input(&data);

        // Keep attachment paths in the provider-independent history. Images
        // are loaded on demand by dream's ViewImage tool.
        debug!(
            attachment_count = data.files.len(),
            "Building structured DreamEngine content blocks"
        );
        let content_blocks = build_content_blocks(&data.content, &data.files);
        debug!(
            block_count = content_blocks.len(),
            "Built structured DreamEngine content blocks"
        );

        // One anchor for both history stores: the conversation-layer turn id
        // is stamped onto this turn's DB message rows (BackendTurnBound →
        // stream persistence, mirroring the codex reader) AND onto the
        // engine's session messages, so an at-turn fork can cut both at the
        // same point.
        let turn_anchor = data
            .turn_id
            .clone()
            .unwrap_or_else(|| format!("turn_{}", generate_short_id()));
        let _ = self
            .runtime
            .event_sender()
            .send(AgentStreamEvent::BackendTurnBound(turn_anchor.clone()));

        let mut engine = self.engine.lock().await;
        engine.set_next_turn_id(Some(turn_anchor));

        let result = tokio::select! {
            res = engine.run_with_blocks(content_blocks, &data.msg_id) => TurnOutcome::Completed(res),
            _ = self.cancel_notify.notified() => {
                info!(
                    conversation_id = %self.runtime.conversation_id(),
                    "DreamEngine engine.run() cancelled by stop signal"
                );
                engine.abort_current_turn("Tool execution canceled by user");
                TurnOutcome::Cancelled
            }
            // Last line of defence. The turn budget bounds how many times the
            // model may act; nothing bounded how long one attempt may hang, so
            // a stalled provider read left the turn running with the user
            // watching a spinner — observed at 183 minutes.
            _ = stalled_for(self.runtime.subscribe(), TURN_IDLE_LIMIT) => {
                warn!(
                    conversation_id = %self.runtime.conversation_id(),
                    idle_secs = TURN_IDLE_LIMIT.as_secs(),
                    "DreamEngine turn produced nothing for the idle limit; aborting"
                );
                engine.abort_current_turn("Turn stalled with no output");
                TurnOutcome::TimedOut
            }
        };

        let elapsed_ms = now_ms() - started_at;
        self.runtime.bump_activity();

        let send_result = match result {
            // The engine reports an exhausted turn budget as a *successful*
            // run with `StopReason::MaxTurns` and an English line on the
            // output stream. Surface it as the structured, localized error it
            // actually is, so it reads the same as the wall-clock stop below
            // instead of arriving as untranslated prose.
            TurnOutcome::Completed(Ok(run_result)) if run_result.stop_reason == StopReason::MaxTurns => {
                warn!(
                    conversation_id = %self.runtime.conversation_id(),
                    elapsed_ms,
                    turns = run_result.turns,
                    "DreamEngine turn stopped at the turn budget"
                );
                self.emit_turn_usage(&engine, Some(&run_result));
                let send_error = turn_limit_send_error(format!(
                    "Stopped after {} agentic turns without converging.",
                    run_result.turns
                ));
                self.runtime.emit_error_data(send_error.stream_error().clone());
                Err(send_error)
            }
            TurnOutcome::Completed(Ok(run_result)) => {
                info!(
                    conversation_id = %self.runtime.conversation_id(),
                    elapsed_ms,
                    "DreamEngine engine.run() completed, emitting Finish"
                );
                // BEFORE the Finish: the relay stops forwarding a turn once it
                // sees Finish, so a usage frame emitted after it is dropped.
                self.emit_turn_usage(&engine, Some(&run_result));
                self.runtime.emit_finish(None);
                Ok(())
            }
            TurnOutcome::TimedOut => {
                // Same accounting as the cancel and error branches: whatever
                // the stalled turn already spent is spent.
                self.emit_turn_usage(&engine, None);
                let send_error = turn_limit_send_error(format!(
                    "Stopped after producing nothing for {} minutes.",
                    TURN_IDLE_LIMIT.as_secs() / 60
                ));
                self.runtime.emit_error_data(send_error.stream_error().clone());
                Err(send_error)
            }
            TurnOutcome::Completed(Err(e)) => {
                let summary = engine_runtime_error_summary(&e);
                error!(
                    conversation_id = %self.runtime.conversation_id(),
                    elapsed_ms,
                    error = %ErrorChain(&e),
                    "DreamEngine engine.run() failed, emitting Error"
                );
                error!(
                    target: "dream_feedback_diagnostics",
                    diagnostic_event = "feedback.runtime.dream_engine_error",
                    conversation_id = %self.runtime.conversation_id(),
                    msg_id = %data.msg_id,
                    turn_id = data.turn_id.as_deref().unwrap_or("none"),
                    elapsed_ms,
                    error_kind = summary.kind,
                    provider_error_class = summary.provider_error_class,
                    http_status = summary.http_status,
                    failure_count = summary.failure_count,
                    failure_limit = summary.failure_limit,
                    "feedback.runtime.dream_engine_error"
                );
                // Occupancy without a breakdown: the turn produced no
                // `AgentResult`, but the tokens it burned before failing are
                // gone all the same, and an indicator frozen at the previous
                // number reads as "that attempt was free".
                self.emit_turn_usage(&engine, None);
                let send_error = engine_error_to_send_error(&e);
                self.runtime.emit_error_data(send_error.stream_error().clone());
                Err(send_error)
            }
            TurnOutcome::Cancelled => {
                // Cancelled mid-turn. Same reasoning as the error branch: the
                // work already sent to the provider was paid for.
                self.emit_turn_usage(&engine, None);
                self.runtime.emit_finish(None);
                Ok(())
            }
        };
        self.turn_finished_notify.notify_waiters();
        send_result
    }

    async fn cancel(&self) -> Result<(), AgentError> {
        self.request_stop(None, "cancel");
        Ok(())
    }

    fn kill(&self, reason: Option<AgentKillReason>) -> Result<(), AgentError> {
        self.request_stop(reason, "kill");
        Ok(())
    }
}

impl DreamEngineAgentManager {
    /// Report the turn's token usage, so the context indicator has something to
    /// show for a dream conversation.
    ///
    /// It had nothing: the only emitter that carries token counts
    /// (`BackendOutputSink::emit_stream_end`) is called exclusively by
    /// `dream-engine-cli` after its own `engine.run()` returns. This path runs
    /// the same engine in-process and converged the turn with
    /// `emit_finish(None)` alone, discarding the `AgentResult` — so the frontend
    /// received a `finish` frame carrying nothing but `session_id` and left the
    /// meter blank, while the engine had the numbers all along.
    ///
    /// Emitted as `AcpContextUsage` rather than as fields on `Finish`. The name
    /// is historical (`broadcast_usage_frame` in `session_agent.rs` says it
    /// "fires for every backend"); the shape `{used, size, _meta}` is the one
    /// the renderer already reads, and unlike `Finish` it can carry a context
    /// WINDOW — which is what turns a raw token count into the percentage the
    /// indicator exists to show.
    ///
    /// ⚠️ `TokenUsage::input_tokens` from the engine is the **full** provider
    /// input, cache reads and cache writes included (see its doc comment). The
    /// renderer's `input_tokens` means the opposite — the fresh input that cache
    /// did NOT cover — because that is what makes a cache hit-rate meaningful
    /// (`cached_read / (cached_read + input)`). Passing the engine's value
    /// through unchanged would double-count the cached tokens and report a hit
    /// rate far below the truth, so the cached parts are subtracted here.
    /// The context snapshot for `GET /api/conversations/:id/usage`.
    ///
    /// Without this the dispatch in `agent_task.rs` returned `None` for this
    /// variant, so the endpoint answered `used: null` for every dream
    /// conversation — and the renderer's hydrate-from-snapshot path (the one
    /// that covers a fresh install, a second machine, or a failed write-back)
    /// had nothing to read. It was one unwritten match arm, not a missing
    /// persistence layer.
    ///
    /// No `_meta`: a snapshot is not a turn, and the last turn's breakdown is
    /// not retained. The live frame carries the breakdown; this carries the
    /// headline the indicator needs to render at all.
    ///
    /// `try_lock` rather than `lock`: the engine mutex is held for the whole
    /// duration of a turn, which can be minutes. Blocking an HTTP handler behind
    /// that to fetch a number the live stream is already delivering would trade
    /// a missing figure for a hung request.
    pub fn context_usage_snapshot(&self) -> Option<Value> {
        let engine = self.engine.try_lock().ok()?;
        let status = engine.context_status();
        // Nothing has been counted yet — the conversation has not run a turn.
        // `None` keeps the endpoint's "no usage" answer rather than reporting a
        // window with zero used, which would render as a real 0% reading.
        if status.context_usage == 0 {
            return None;
        }
        Some(build_turn_usage_frame(
            status.context_usage,
            status.context_window,
            None,
        ))
    }

    fn emit_turn_usage(&self, engine: &AgentEngine, result: Option<&AgentResult>) {
        let status = engine.context_status();
        let usage = result.map(|r| &r.usage);
        let frame = build_turn_usage_frame(status.context_usage, status.context_window, usage);

        debug!(
            conversation_id = %self.runtime.conversation_id(),
            context_usage = status.context_usage,
            context_window = status.context_window,
            input_tokens = usage.map(|u| u.input_tokens),
            output_tokens = usage.map(|u| u.output_tokens),
            cache_read = usage.map(|u| u.cache_read_tokens),
            cache_write = usage.map(|u| u.cache_creation_tokens),
            "DreamEngine turn usage reported"
        );

        self.runtime.emit(AgentStreamEvent::AcpContextUsage(frame));
    }

    pub fn kill_and_wait(&self, reason: Option<AgentKillReason>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let was_running = self.request_stop(reason, "kill");
        let turn_finished_notify = Arc::clone(&self.turn_finished_notify);
        let runtime = self.runtime.clone();
        let conversation_id = self.runtime.conversation_id().to_owned();

        Box::pin(async move {
            if was_running
                && timeout(Duration::from_secs(5), async {
                    while runtime.status() == Some(ConversationStatus::Running) {
                        turn_finished_notify.notified().await;
                    }
                })
                .await
                .is_err()
            {
                warn!(
                    conversation_id,
                    "Timed out waiting for the DreamEngine turn to finish after kill"
                );
            }
        })
    }
}

/// DreamEngine-specific operations reached through `AgentInstance::DreamEngine(..)`
/// matches in the routes + services.
impl DreamEngineAgentManager {
    pub fn confirm(&self, _msg_id: &str, call_id: &str, data: Value, always_allow: bool) -> Result<(), AgentError> {
        if let Ok(mut confs) = self.confirmations.write() {
            confs.retain(|c| c.call_id != call_id);
        }

        let value = data.get("value").and_then(|v| v.as_str()).unwrap_or("cancel");

        let is_cancel = value == "cancel";

        debug!(
            conversation_id = %self.runtime.conversation_id(),
            call_id,
            value,
            always_allow,
            "DreamEngine confirm"
        );

        if is_cancel {
            self.approval_manager.resolve(
                call_id,
                ToolApprovalResult::Denied {
                    reason: "User denied the tool request".into(),
                },
            );
        } else {
            let scope = if always_allow {
                ApprovalScope::Always
            } else {
                ApprovalScope::Once
            };
            self.approval_manager.approve(call_id, scope);
        }
        Ok(())
    }

    pub fn get_confirmations(&self) -> Vec<Confirmation> {
        self.confirmations.read().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn check_approval(&self, action: &str, _command_type: Option<&str>) -> bool {
        self.approval_manager.is_auto_approved(action)
    }

    pub async fn mode(&self) -> Result<AgentModeResponse, AgentError> {
        Ok(AgentModeResponse {
            mode: self.approval_manager.current_mode(),
            initialized: true,
        })
    }

    pub async fn set_mode(&self, mode: &str) -> Result<(), AgentError> {
        let prev = self.approval_manager.current_mode();
        self.approval_manager.set_mode(parse_session_mode(mode));
        info!(
            conversation_id = %self.runtime.conversation_id(),
            from = prev,
            to = mode,
            "DreamEngine session mode switched"
        );
        Ok(())
    }

    pub async fn config_options(&self) -> Result<GetConfigOptionsResponse, AgentError> {
        Ok(GetConfigOptionsResponse {
            config_options: vec![dream_engine_mode_config_option(self.approval_manager.current_mode())],
        })
    }

    pub async fn set_config_option(&self, option_id: &str, value: &str) -> Result<SetConfigOptionResponse, AgentError> {
        let option_id = option_id.trim();
        let value = value.trim();

        if option_id != AIONRS_MODE_OPTION_ID {
            return Err(AgentError::bad_request(format!(
                "Config option '{option_id}' is not available"
            )));
        }
        if !is_dream_engine_session_mode(value) {
            return Err(AgentError::bad_request(format!(
                "Value '{value}' is not selectable for config option '{option_id}'"
            )));
        }

        self.set_mode(value).await?;
        Ok(SetConfigOptionResponse {
            confirmation: ConfigOptionConfirmation::Observed,
            config_options: Some(self.config_options().await?.config_options),
        })
    }

    pub async fn get_slash_commands(&self) -> Result<Vec<SlashCommandItem>, AgentError> {
        Ok(self.slash_commands.clone())
    }
}

const AIONRS_MODE_OPTION_ID: &str = "mode";

fn is_dream_engine_session_mode(s: &str) -> bool {
    matches!(s, "default" | "auto_edit" | "yolo")
}

fn dream_engine_mode_config_option(current_value: String) -> AcpConfigOptionDto {
    AcpConfigOptionDto {
        id: AIONRS_MODE_OPTION_ID.to_owned(),
        name: Some("Mode".to_owned()),
        label: None,
        description: None,
        category: Some("mode".to_owned()),
        option_type: "select".to_owned(),
        current_value: Some(current_value),
        options: vec![
            aionrs_mode_select_option("default", "Default"),
            aionrs_mode_select_option("auto_edit", "Auto Edit"),
            aionrs_mode_select_option("yolo", "YOLO"),
        ],
    }
}

fn aionrs_mode_select_option(value: &str, name: &str) -> AcpConfigSelectOptionDto {
    AcpConfigSelectOptionDto {
        value: value.to_owned(),
        name: Some(name.to_owned()),
        label: None,
        description: None,
    }
}

fn parse_session_mode(s: &str) -> SessionMode {
    match s {
        "auto_edit" => SessionMode::AutoEdit,
        "yolo" => SessionMode::Yolo,
        _ => SessionMode::Default,
    }
}

#[cfg(test)]
#[path = "agent_test.rs"]
mod agent_test;
