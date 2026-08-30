use std::sync::Arc;

use dream_core_ai_agent::types::{BuildTaskOptions, SendMessageData};
use dream_core_ai_agent::{
    AgentError, AgentInstance, AgentSendError, AgentSessionContext, AgentSessionKind, IWorkerTaskManager,
    RequiredFullAutoApplication,
};
use dream_core_common::{AgentType, ConversationStatus, ErrorChain, now_ms};
use dream_core_db::models::ConversationRow;
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

use crate::agent_health_policy::{AgentHealthAction, AgentHealthPolicy};
use crate::runtime_state::RuntimeLifecycleState;
use crate::runtime_state::TurnClaim;
use crate::service::{
    ConversationService, MAX_SYSTEM_RESPONSE_CONTINUATIONS_PER_TURN, agent_error_top_level_code, persist_session_key,
};
use crate::stream_relay::{RelayOutcome, StreamRelay, SupersedingTipTotals, TurnAttemptSummary};
use crate::turn_continuation_policy::{ContinuationDecision, TurnContinuationPolicy};
use crate::turn_recovery_policy::{TurnRecoveryDecision, TurnRecoveryPolicy};
use dream_core_api_types::AgentErrorCode;

fn acp_backend_from_build_options(options: &BuildTaskOptions) -> Option<&str> {
    match &options.context.kind {
        AgentSessionKind::Acp(ctx) => ctx.config.backend.as_deref(),
        AgentSessionKind::Antigravity(ctx) => ctx.config.backend.as_deref(),
        AgentSessionKind::DreamEngine(_) => None,
    }
}

/// The channel attribution for this attempt's billing row (P1-4): the raw
/// `providers.id` of the configuration the turn was built for —
/// `prov_chan_<channel_id>` for enterprise channels, an opaque id for personal
/// provider configs. Extracted BEFORE `build_options` is moved into
/// `get_or_build_task` (same borrow-before-move pattern as
/// `acp_backend_from_build_options`), because after the move the session
/// context is gone and there is no second chance to read it.
///
/// An empty/whitespace id counts as no channel: the column stays NULL and the
/// report buckets the row as `unknown`, rather than an empty-string key that
/// would render as a blank channel in the console.
fn channel_id_from_build_options(options: &BuildTaskOptions) -> Option<String> {
    let id = options.context.model.provider_id.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

pub(crate) struct TurnStartInput {
    pub user_id: String,
    pub conversation: ConversationRow,
    /// User message content, already resolved to the inlined `[[AION_FILES]]`
    /// form (HTTP path resolves `ChatFileRef`s; internal agent turns pass a
    /// pre-formed string).
    pub content: String,
    /// Attachment absolute paths, already resolved.
    pub files: Vec<String>,
    pub inject_skills: Vec<String>,
    pub required_runtime_mode: Option<String>,
    pub build_options: BuildTaskOptions,
    pub stored_workspace: String,
    pub turn_id: String,
    pub turn_claim: TurnClaim,
    /// True when `content` is an automation-built prompt (cron, team dispatch)
    /// rather than something a human typed. Consumed only by the P2-2 memory
    /// extractor, which must not mine a synthetic prompt for "user facts".
    pub synthetic_prompt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversationTurnStatus {
    Completed,
    Failed,
}

pub(crate) struct ConversationTurnResult {
    pub status: ConversationTurnStatus,
    pub error_message: Option<String>,
}

pub(crate) struct ConversationTurnOrchestrator {
    service: ConversationService,
    task_manager: Arc<dyn IWorkerTaskManager>,
}

struct TurnAttemptInput {
    conv_id: String,
    turn_id: String,
    user_id: String,
    build_options: BuildTaskOptions,
    stored_workspace: String,
    send: SendMessageData,
    msg_id: String,
    allowed_skill_names: Vec<String>,
    required_runtime_mode: Option<String>,
    continuation_count: usize,
    defer_clean_terminal_errors: bool,
    /// Shared by every attempt of this turn — see the field's use in run_attempt.
    superseding_tips: SupersedingTipTotals,
}

struct TurnAttemptResult {
    outcome: RelayOutcome,
    summary: TurnAttemptSummary,
    agent_type: AgentType,
    backend: Option<String>,
}

impl ConversationTurnOrchestrator {
    pub fn new(service: ConversationService, task_manager: Arc<dyn IWorkerTaskManager>) -> Self {
        Self { service, task_manager }
    }

    pub fn spawn_user_turn(self, input: TurnStartInput) {
        tokio::spawn(async move {
            let _ = self.run_user_turn(input).await;
        });
    }

    async fn run_attempt(&self, input: TurnAttemptInput) -> Result<TurnAttemptResult, ConversationTurnResult> {
        let build_started_at = now_ms();
        let availability_agent_id = availability_agent_id(&input.build_options);
        let backend = acp_backend_from_build_options(&input.build_options).map(str::to_owned);
        // Borrowed before `build_options` is moved into `get_or_build_task` —
        // see `channel_id_from_build_options` for why there is no later chance.
        let channel_id = channel_id_from_build_options(&input.build_options);
        info!(
            conversation_id = %input.conv_id,
            turn_id = %input.turn_id,
            "Agent task build started"
        );

        let agent = match self
            .task_manager
            .get_or_build_task(&input.conv_id, input.build_options)
            .await
        {
            Ok(agent) => agent,
            Err(err) => {
                let top_level_code = agent_error_top_level_code(&err);
                let send_error = AgentSendError::from_agent_error_ref_for_backend(&err, backend.as_deref());
                let top_level_code = if send_error.is_openclaw_gateway_unreachable() {
                    "USER_AGENT_OPENCLAW_GATEWAY_UNREACHABLE"
                } else {
                    top_level_code
                };
                if send_error.is_openclaw_gateway_unreachable() {
                    warn!(
                        conversation_id = %input.conv_id,
                        turn_id = %input.turn_id,
                        backend = "openclaw",
                        error_kind = "openclaw_gateway_unreachable",
                        port = 18789_u16,
                        phase = "turn_build",
                        "OpenClaw Gateway unreachable during ACP startup"
                    );
                }
                error!(
                    conversation_id = %input.conv_id,
                    turn_id = %input.turn_id,
                    error_code = ?send_error.code(),
                    error = %ErrorChain(&err),
                    "Agent task build failed"
                );
                let failure_message = send_error_display_message(&send_error);
                // A build that failed because the agent disowned our session id
                // must not leave that id behind: warmup replays it on every later
                // turn and fails the same way BEFORE the prompt is sent, so the
                // real cause is never reachable again. Verified live (conversation
                // 161c458a): the agent was killed after a first error, its
                // in-process session died with it, and the id then produced
                // `Session not found` on every turn until the row was cleared.
                //
                // This is the build path — the terminal-error eviction in
                // `evict_acp_task_after_terminal_error` never runs here, which is
                // why clearing there alone did not fix it.
                self.service
                    .clear_persisted_acp_session_after_disown(&input.user_id, &input.conv_id, send_error.code())
                    .await;
                record_agent_session_failure(
                    &self.service,
                    &input.user_id,
                    availability_agent_id.as_deref(),
                    "session_build_failed",
                    &failure_message,
                )
                .await;
                self.service
                    .persist_and_broadcast_send_failure_tip(
                        &input.user_id,
                        &input.conv_id,
                        &input.turn_id,
                        &send_error,
                        Some(top_level_code),
                    )
                    .await;
                return Err(ConversationTurnResult {
                    status: ConversationTurnStatus::Failed,
                    error_message: Some(failure_message),
                });
            }
        };

        if let Err(err) = self
            .service
            .maybe_persist_workspace(
                &input.user_id,
                &input.conv_id,
                &input.stored_workspace,
                agent.workspace(),
            )
            .await
        {
            let top_level_code = err.error_code();
            let failure_message = err.to_string();
            let send_error = AgentSendError::from_agent_error(err.to_agent_error());
            error!(
                conversation_id = %input.conv_id,
                turn_id = %input.turn_id,
                error_code = err.error_code(),
                error = %ErrorChain(&err),
                "Failed to persist resolved workspace"
            );
            self.service
                .persist_and_broadcast_send_failure_tip(
                    &input.user_id,
                    &input.conv_id,
                    &input.turn_id,
                    &send_error,
                    Some(top_level_code),
                )
                .await;
            return Err(ConversationTurnResult {
                status: ConversationTurnStatus::Failed,
                error_message: Some(failure_message),
            });
        }

        info!(
            conversation_id = %input.conv_id,
            turn_id = %input.turn_id,
            agent_type = ?agent.agent_type(),
            elapsed_ms = now_ms().saturating_sub(build_started_at),
            "Agent task ready"
        );

        // The send path builds the agent HERE, not through the service's
        // ensure-runtime fn — without this the out-of-turn watcher only existed
        // for conversations opened via that other path (found live: a codex e2e
        // probe ran with no watcher at all, and its post-finish MessageDeltas
        // were dropped exactly like the pre-watcher days).
        self.service
            .ensure_background_watcher(&input.user_id, &input.conv_id, &agent);

        let persistence = self.service.runtime_persistence();
        let runtime_state = self.service.runtime_state();

        // A cancel may have arrived while the task was still being built, when
        // there was no agent to hand it to (see `ConversationService::cancel`).
        // Honour it here, BEFORE the prompt goes out: the user withdrew this
        // turn, so the cleanest outcome is that the CLI never runs at all.
        //
        // Ends as Completed, not Failed — a turn the user cancelled is not an
        // error, and reporting one would surface a red bubble for something
        // they asked for.
        if runtime_state.take_deferred_cancel(&input.conv_id, &input.turn_id) {
            info!(
                conversation_id = %input.conv_id,
                turn_id = %input.turn_id,
                "Applying a cancel that arrived while the agent was still being built"
            );
            // Tell the UI the turn is over. Every OTHER terminal on this path is
            // emitted by the `StreamRelay`, which is built further down — so
            // returning here without a frame settles the turn on the server and
            // leaves the client's spinner running until the 15s watchdog.
            //
            // Live symptom (agy, 2026-08-12): the conversation produced NO
            // stream frames at all, not even `start`, and
            // `live_antigravity_cancel_settles_and_recovers` failed 4/4. agy is
            // where it shows up because its build is the slowest — probing
            // models, checking the CLI version, installing the permission hook
            // and writing the MCP config — so a cancel lands inside it rather
            // than after it. Nothing about the bug is agy-specific.
            self.service.broadcast_turn_settled_without_relay(
                &input.user_id,
                &input.conv_id,
                &input.turn_id,
                &input.msg_id,
            );
            return Err(ConversationTurnResult {
                status: ConversationTurnStatus::Completed,
                error_message: None,
            });
        }

        let mut pending_send = Some((input.send, input.msg_id));
        let mut continuation_count = input.continuation_count;
        let continuation_policy = TurnContinuationPolicy::new(MAX_SYSTEM_RESPONSE_CONTINUATIONS_PER_TURN);
        let mut last_outcome = None;
        let mut aggregate_summary = TurnAttemptSummary::default();

        while let Some((current_send, msg_id)) = pending_send.take() {
            // P1-3 latency collection: each loop pass is one real model call
            // (the same granularity `meter_attempt` bills at), so the wall
            // clock starts here — covering build reuse, the send, and the full
            // stream drain — and is read at the `trace_attempt` call below.
            let attempt_started = std::time::Instant::now();
            let lifecycle = runtime_state.lifecycle_for(&input.conv_id);
            let defer_clean_terminal_errors = input.defer_clean_terminal_errors
                && agent.agent_type() == AgentType::Acp
                && lifecycle == RuntimeLifecycleState::Active
                && aggregate_summary.safe_to_auto_replay();
            let relay = StreamRelay::new(
                input.conv_id.clone(),
                msg_id,
                input.turn_id.clone(),
                input.user_id.clone(),
                self.service.conversation_repo().clone(),
                self.service.broadcaster().clone(),
            )
            .with_skill_resolver(self.service.skill_resolver())
            .with_allowed_skill_names(input.allowed_skill_names.clone())
            .with_runtime_state(Arc::clone(&runtime_state))
            .with_persistence(persistence.clone())
            .with_turn_completion(false)
            .with_defer_clean_terminal_errors(defer_clean_terminal_errors)
            // A replay spawns a fresh CLI whose own retry counter starts at one,
            // but from the user's side it is still the same stalled prompt and
            // the same card counting up — so these totals span the attempts.
            .with_superseding_tip_totals(input.superseding_tips.clone());

            let rx = agent.subscribe();
            if let Some(mode) = input
                .required_runtime_mode
                .as_deref()
                .map(str::trim)
                .filter(|mode| !mode.is_empty())
            {
                match apply_required_runtime_mode(&agent, backend.as_deref(), mode).await {
                    Ok(RequiredFullAutoApplication::Applied { effective }) => {
                        info!(
                            conversation_id = %input.conv_id,
                            turn_id = %input.turn_id,
                            mode,
                            "Confirmed required runtime mode before agent turn"
                        );
                        if effective != mode {
                            info!(
                                requested = %mode,
                                effective = %effective,
                                backend = ?backend,
                                conversation_id = %input.conv_id,
                                "cron required mode remapped to backend-native full-auto"
                            );
                        }
                    }
                    Ok(RequiredFullAutoApplication::Skipped { resolved }) => {
                        // look-before-leap: the resolved native YOLO is not
                        // selectable on this backend. The sunk method already
                        // logged a warn with backend/conversation_id/resolved;
                        // keep the session's already-resolved mode and continue
                        // this turn rather than failing the whole turn.
                        info!(
                            conversation_id = %input.conv_id,
                            turn_id = %input.turn_id,
                            mode,
                            resolved = %resolved,
                            "Skipped required runtime mode override — keeping session mode"
                        );
                    }
                    Err(err) => {
                        let top_level_code = agent_error_top_level_code(&err);
                        let failure_message = err.to_string();
                        let send_error = AgentSendError::from_agent_error(err);
                        error!(
                            conversation_id = %input.conv_id,
                            turn_id = %input.turn_id,
                            mode,
                            error = %failure_message,
                            "Failed to apply required runtime mode before agent turn"
                        );
                        self.service
                            .persist_and_broadcast_send_failure_tip(
                                &input.user_id,
                                &input.conv_id,
                                &input.turn_id,
                                &send_error,
                                Some(top_level_code),
                            )
                            .await;
                        return Err(ConversationTurnResult {
                            status: ConversationTurnStatus::Failed,
                            error_message: Some(failure_message),
                        });
                    }
                }
            }
            let send_agent = agent.clone();
            let conv_id_send = input.conv_id.clone();
            let turn_id_for_send = input.turn_id.clone();
            let feedback_user_id = input.user_id.clone();
            let feedback_service = self.service.clone();
            let feedback_agent_id = availability_agent_id.clone();
            let (send_error_tx, send_error_rx) = oneshot::channel();

            tokio::spawn(async move {
                if let Err(e) = send_agent.send_message(current_send).await {
                    let failure_message = send_error_display_message(&e);
                    record_agent_session_failure(
                        &feedback_service,
                        &feedback_user_id,
                        feedback_agent_id.as_deref(),
                        "session_send_failed",
                        &failure_message,
                    )
                    .await;
                    let task_status = send_agent.status();
                    let agent_type = send_agent.agent_type();
                    error!(
                        conversation_id = %conv_id_send,
                        turn_id = %turn_id_for_send,
                        ?agent_type,
                        ?task_status,
                        error = %ErrorChain(&e),
                        "Agent send_message failed"
                    );
                    if task_status == Some(ConversationStatus::Finished) {
                        debug!(
                            conversation_id = %conv_id_send,
                            turn_id = %turn_id_for_send,
                            ?agent_type,
                            "Agent send_message failed on finished task; relay will prefer any runtime terminal before fallback"
                        );
                    }
                    warn!(
                        conversation_id = %conv_id_send,
                        turn_id = %turn_id_for_send,
                        ?agent_type,
                        code = ?e.code(),
                        ownership = ?e.ownership(),
                        "Agent send_message returned error; offering fallback stream error to relay"
                    );
                    let _ = send_error_tx.send(e);
                }
            });

            let outcome = relay.consume_with_send_error(rx, send_error_rx).await;
            aggregate_summary.merge(&outcome.attempt);

            // P0-3 usage metering: one call per attempt that actually ran a
            // turn — a "send" that continues (system-response auto-replies)
            // makes several real LLM calls, each with its own cost, so this
            // fires inside the loop rather than once after it. Cost is
            // recorded even when the terminal was an error: dream reports
            // real token counts whenever it reports any, regardless of
            // whether the turn went on to fail — those tokens were still
            // billed by the provider.
            let recorder = self.service.usage_recorder.read().ok().and_then(|g| g.clone());
            if let Some(recorder) = recorder {
                meter_attempt(
                    recorder.as_ref(),
                    &input.user_id,
                    &input.conv_id,
                    channel_id.as_deref(),
                    &outcome,
                );
            }

            // P2-5 per-call trace: same boundary, finer grain — every
            // attempt (succeeded or not) and every tool delegate lands as
            // its own row, so an administrator can see retries and failures
            // the per-turn aggregate folds away.
            let trace_recorder = self.service.llm_trace_recorder.read().ok().and_then(|g| g.clone());
            if let Some(recorder) = trace_recorder {
                let duration_ms = i64::try_from(attempt_started.elapsed().as_millis()).ok();
                trace_attempt(recorder.as_ref(), &input.user_id, &input.conv_id, &outcome, duration_ms);
            }

            if let Some(session_key) = agent.get_session_key() {
                persist_session_key(
                    self.service.conversation_repo(),
                    &persistence,
                    &input.user_id,
                    &input.conv_id,
                    &session_key,
                )
                .await;
            }

            match continuation_policy.decide(&input.conv_id, continuation_count, &outcome, lifecycle) {
                ContinuationDecision::Continue { content, next_count } => {
                    continuation_count = next_count;
                    let next_turn_msg_id = ConversationService::mint_msg_id();
                    pending_send = Some((
                        SendMessageData {
                            content,
                            msg_id: next_turn_msg_id.clone(),
                            turn_id: Some(input.turn_id.clone()),
                            files: vec![],
                            inject_skills: vec![],
                        },
                        next_turn_msg_id,
                    ));
                }
                ContinuationDecision::Stop(_) => {
                    last_outcome = Some(outcome);
                    break;
                }
            }
        }

        Ok(TurnAttemptResult {
            outcome: last_outcome.unwrap_or_default(),
            summary: aggregate_summary,
            agent_type: agent.agent_type(),
            backend,
        })
    }

    pub(crate) async fn run_user_turn(self, mut input: TurnStartInput) -> ConversationTurnResult {
        let mut turn_claim = input.turn_claim;
        let conv_id = input.conversation.id.clone();
        let turn_id = input.turn_id.clone();
        let runtime_state = self.service.runtime_state();
        let allowed_skill_names = input.build_options.context.skills.clone();
        let first_turn_msg_id = ConversationService::mint_msg_id();

        // P2-2 memory injection — prepend the caller's relevant readable
        // memory to this turn's preset context, before any attempt is built.
        // Best-effort and hard-bounded: an unwired seam, an error, or a slow
        // lookup all leave the context exactly as it was. `preset_context`
        // (ACP/agy) only actually reaches the model on a session's first
        // turn, so in practice this loads accumulated memory when a member
        // opens a new conversation — the same lifetime as "[Assistant Rules]".
        let user_message = input.content.clone();
        if let Some(provider) = self.service.memory_context_provider.read().ok().and_then(|g| g.clone()) {
            let recall = tokio::time::timeout(
                std::time::Duration::from_millis(300),
                provider.recall(&input.user_id, &user_message),
            )
            .await
            .unwrap_or_default();
            if !recall.is_empty() {
                let block = format!("[Relevant Memory]\n{}\n[/Relevant Memory]", recall.join("\n"));
                prepend_preset_context(&mut input.build_options.context, &block);
            }
        }

        let initial_send = SendMessageData {
            content: input.content,
            msg_id: first_turn_msg_id.clone(),
            turn_id: Some(turn_id.clone()),
            files: input.files,
            inject_skills: input.inject_skills,
        };
        let mut replayed = false;
        let superseding_tips = SupersedingTipTotals::default();
        let mut replay_started_at = None;
        let mut final_error_message;
        let mut auth_failure = false;

        info!(conversation_id = %conv_id, turn_id = %turn_id, "conversation turn orchestrator started");

        let final_failed = loop {
            let attempt_number = if replayed { 2 } else { 1 };
            let attempt_result = match self
                .run_attempt(TurnAttemptInput {
                    conv_id: conv_id.clone(),
                    turn_id: turn_id.clone(),
                    user_id: input.user_id.clone(),
                    build_options: input.build_options.clone(),
                    stored_workspace: input.stored_workspace.clone(),
                    send: initial_send.clone(),
                    msg_id: first_turn_msg_id.clone(),
                    allowed_skill_names: allowed_skill_names.clone(),
                    required_runtime_mode: input.required_runtime_mode.clone(),
                    continuation_count: 0,
                    defer_clean_terminal_errors: !replayed,
                    superseding_tips: superseding_tips.clone(),
                })
                .await
            {
                Ok(result) => result,
                Err(result) => {
                    final_error_message = result.error_message;
                    break result.status == ConversationTurnStatus::Failed;
                }
            };

            // Track the final attempt's auth signal so the post-loop availability
            // write-back can reflect "needs sign-in" (last iteration wins).
            auth_failure = terminal_is_auth_failure(&attempt_result.outcome);

            let lifecycle = runtime_state.lifecycle_for(&conv_id);
            if !attempt_result.outcome.terminal.is_error() {
                final_error_message = None;
                if replayed {
                    info!(
                        conversation_id = %conv_id,
                        turn_id = %turn_id,
                        attempt = attempt_number,
                        elapsed_ms = replay_started_at
                            .map(|started_at| now_ms().saturating_sub(started_at))
                            .unwrap_or_default(),
                        "conversation turn auto replay completed"
                    );
                }
                break false;
            }
            final_error_message = turn_attempt_error_message(&attempt_result.summary);
            if replayed {
                warn!(
                    conversation_id = %conv_id,
                    turn_id = %turn_id,
                    attempt = attempt_number,
                    error_code = ?attempt_result.outcome.terminal.code(),
                    retryable = ?attempt_result.outcome.terminal.retryable(),
                    "conversation turn auto replay failed"
                );
            }

            let mut recovery_outcome = attempt_result.outcome.clone();
            recovery_outcome.attempt = attempt_result.summary.clone();
            let decision = TurnRecoveryPolicy::decide(
                attempt_result.agent_type,
                attempt_result.backend.as_deref(),
                &recovery_outcome,
                lifecycle,
                replayed,
            );

            match decision {
                TurnRecoveryDecision::AutoReplayOnce { reason, .. } => {
                    replay_started_at = Some(now_ms());
                    info!(
                        conversation_id = %conv_id,
                        turn_id = %turn_id,
                        attempt = attempt_number,
                        next_attempt = attempt_number + 1,
                        backend = attempt_result.backend.as_deref().unwrap_or("unknown"),
                        error_code = ?attempt_result.outcome.terminal.code(),
                        retryable = ?attempt_result.outcome.terminal.retryable(),
                        ?reason,
                        "conversation turn auto replay starting"
                    );
                    self.service
                        .evict_acp_task_after_terminal_error(
                            &input.user_id,
                            &conv_id,
                            attempt_result.agent_type,
                            &attempt_result.outcome,
                            &self.task_manager,
                        )
                        .await;
                    // ELECTRON-3Q0: attempt 1's dead-anchor self-heal cleared
                    // `acp_session.session_id` mid-turn (persist_side_effects runs
                    // BEFORE the terminal reaches this loop). The turn-start
                    // snapshot still holds the stale anchor — refresh ONLY the
                    // anchor fields so the rebuilt task opens Fresh instead of
                    // re-resuming the same dead session.
                    self.service
                        .refresh_resume_anchor_for_replay(&conv_id, &mut input.build_options)
                        .await;
                    replayed = true;
                    continue;
                }
                TurnRecoveryDecision::None => {
                    if attempt_result.outcome.attempt.terminal_error_deferred
                        && let Some(data) = attempt_result.outcome.attempt.terminal_error.clone()
                    {
                        let send_error = AgentSendError::from_stream_error_data(data);
                        self.service
                            .persist_and_broadcast_send_failure_tip(
                                &input.user_id,
                                &conv_id,
                                &turn_id,
                                &send_error,
                                None,
                            )
                            .await;
                    }

                    match AgentHealthPolicy::decide(attempt_result.agent_type, &attempt_result.outcome, lifecycle) {
                        AgentHealthAction::Keep => {}
                        AgentHealthAction::EvictAcpTask { .. } => {
                            self.service
                                .evict_acp_task_after_terminal_error(
                                    &input.user_id,
                                    &conv_id,
                                    attempt_result.agent_type,
                                    &attempt_result.outcome,
                                    &self.task_manager,
                                )
                                .await;
                        }
                    }
                    break true;
                }
            }
        };

        if auth_failure {
            // The agent connected (detection saw it online) but a real turn hit
            // an explicit auth signal — write "needs sign-in" back to its
            // availability so the list stops showing it as plainly usable.
            record_agent_session_failure(
                &self.service,
                &input.user_id,
                availability_agent_id(&input.build_options).as_deref(),
                "auth_required",
                final_error_message
                    .as_deref()
                    .unwrap_or("Agent requires sign-in to run."),
            )
            .await;
        } else if !final_failed {
            record_agent_session_success(
                &self.service,
                &input.user_id,
                availability_agent_id(&input.build_options).as_deref(),
            )
            .await;
        }

        let was_deleting = turn_claim.release_for_turn(&turn_id);
        self.service
            .complete_released_turn(&input.user_id, &conv_id, &turn_id, was_deleting)
            .await;

        // P2-2 memory extraction — fire-and-forget on a turn that produced a
        // reply. The extractor spawns its own work and reads the assistant
        // side from persistence; a failure here must never touch this turn's
        // result. Skipped for a synthetic (cron/dispatch) prompt.
        if !final_failed {
            if let Some(extractor) = self.service.turn_memory_extractor.read().ok().and_then(|g| g.clone()) {
                extractor.extract_from_turn(
                    input.user_id.clone(),
                    conv_id.clone(),
                    user_message,
                    input.synthetic_prompt,
                );
            }
        }

        ConversationTurnResult {
            status: if final_failed {
                ConversationTurnStatus::Failed
            } else {
                ConversationTurnStatus::Completed
            },
            error_message: if final_failed { final_error_message } else { None },
        }
    }
}

/// Prepend `block` to whichever preset field this session kind carries —
/// `preset_context` for ACP / Antigravity, `preset_rules` for DreamEngine —
/// keeping any existing value after a blank line. Used only by P2-2 memory
/// injection.
fn prepend_preset_context(context: &mut AgentSessionContext, block: &str) {
    fn joined(block: &str, existing: Option<String>) -> Option<String> {
        Some(match existing {
            Some(prev) if !prev.trim().is_empty() => format!("{block}\n\n{prev}"),
            _ => block.to_owned(),
        })
    }
    match &mut context.kind {
        AgentSessionKind::Acp(c) => {
            c.config.preset_context = joined(block, c.config.preset_context.take());
        }
        AgentSessionKind::Antigravity(c) => {
            c.config.preset_context = joined(block, c.config.preset_context.take());
        }
        AgentSessionKind::DreamEngine(c) => {
            c.config.preset_rules = joined(block, c.config.preset_rules.take());
        }
    }
}

fn availability_agent_id(options: &BuildTaskOptions) -> Option<String> {
    match &options.context.kind {
        AgentSessionKind::Acp(context) => context
            .config
            .agent_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        AgentSessionKind::Antigravity(context) => context
            .config
            .agent_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        AgentSessionKind::DreamEngine(_) => None,
    }
}

/// True when the turn's terminal is an explicit authentication signal: an
/// `ACP_EMPTY_TURN_NEEDS_AUTH` benign tip (the agent connected but isn't signed
/// in and returned an empty end_turn), or an Error terminal carrying an
/// auth/login error code. Used to reflect "needs sign-in" into the agent's
/// availability even when detection (initialize + session/new, no prompt)
/// showed it online. Non-auth outcomes — generic empty turns, billing,
/// rate-limit, context, network — are deliberately excluded so we don't flip an
/// agent to unavailable for transient or unrelated failures.
fn terminal_is_auth_failure(outcome: &RelayOutcome) -> bool {
    if outcome.attempt.needs_auth {
        return true;
    }
    matches!(
        outcome.terminal.code(),
        Some(
            AgentErrorCode::UserAgentAuthRequired
                | AgentErrorCode::UserLlmProviderAuthFailed
                | AgentErrorCode::UserLlmProviderAwsSsoExpired
        )
    )
}

/// codex's canonical full-access mode id (migration 021 / `full_auto_mode_id`,
/// `dream_core_common::enums`). What cron's forced full-auto — and a resumed full-access
/// session — persist as `required_runtime_mode`.
const CODEX_CANONICAL_FULL_ACCESS_MODE: &str = "agent-full-access";
/// The LEGACY bare token codex's LIVE catalog actually advertises for the full-access
/// tier (`:danger-full-access`→`full-access`, codex_conn `fill_discovery` /
/// `profile_id_to_legacy_value`).
const CODEX_LEGACY_FULL_ACCESS_MODE: &str = "full-access";

/// ELECTRON-3Q0: align codex's persisted canonical full-access id to whatever the
/// LIVE catalog exposes, mirroring what AcpAgentManager reconcile already does via
/// `normalize_requested_mode_for_available_values`. codex persists the canonical
/// `agent-full-access` (cron forces it for scheduled tasks) but codex's live catalog
/// advertises the legacy bare token `full-access`, so the unnormalized apply hit
/// `set_config_option`'s REJECT ("mode 'agent-full-access' is not one of the available
/// modes") and failed every cron turn before the send.
///
/// Narrow by design (only the required-mode auto-apply path calls this): downgrade
/// ONLY the canonical full-access id, ONLY when the live catalog lacks it but does
/// carry the legacy token. Every other case returns the value unchanged — a live
/// catalog that carries the canonical id keeps it, and a catalog with NO full-access
/// tier at all leaves the value so `set_config_option` still REJECTs (never silently
/// pick a weaker mode). Non-codex backends and non-full-access modes pass straight
/// through. An empty catalog is left untouched too (set_config_option is permissive
/// on an empty/not-yet-discovered catalog).
fn normalize_required_mode_for_catalog(backend: Option<&str>, mode: &str, available_ids: &[String]) -> String {
    if backend != Some("codex") || mode != CODEX_CANONICAL_FULL_ACCESS_MODE {
        return mode.to_owned();
    }
    let has_canonical = available_ids.iter().any(|id| id == CODEX_CANONICAL_FULL_ACCESS_MODE);
    let has_legacy = available_ids.iter().any(|id| id == CODEX_LEGACY_FULL_ACCESS_MODE);
    if !has_canonical && has_legacy {
        return CODEX_LEGACY_FULL_ACCESS_MODE.to_owned();
    }
    mode.to_owned()
}

/// Read the live mode-catalog ids for normalization. Only consulted when a codex
/// canonical full-access id is being applied (see `resolve_required_runtime_mode`);
/// a catalog read failure returns None so the caller falls back to the requested mode
/// unnormalized (= pre-fix behavior).
async fn live_mode_catalog_ids(agent: &AgentInstance) -> Option<Vec<String>> {
    match agent.get_config_options().await {
        Ok(resp) => Some(
            resp.config_options
                .into_iter()
                .find(|opt| opt.id == "mode")
                .map(|opt| opt.options.into_iter().map(|o| o.value).collect())
                .unwrap_or_default(),
        ),
        Err(err) => {
            warn!(
                error = %err,
                "apply mode: live catalog read failed — applying the requested mode unnormalized"
            );
            None
        }
    }
}

/// Resolve the mode to actually apply. For codex's canonical full-access id, align it
/// to the live catalog (ELECTRON-3Q0); every other backend/mode is returned verbatim
/// without touching the catalog.
async fn resolve_required_runtime_mode(agent: &AgentInstance, backend: Option<&str>, mode: &str) -> String {
    if backend != Some("codex") || mode != CODEX_CANONICAL_FULL_ACCESS_MODE {
        return mode.to_owned();
    }
    let Some(available_ids) = live_mode_catalog_ids(agent).await else {
        return mode.to_owned();
    };
    let effective = normalize_required_mode_for_catalog(backend, mode, &available_ids);
    if effective != mode {
        info!(
            requested = mode,
            effective = %effective,
            "apply mode: aligned codex canonical full-access to the live catalog (ELECTRON-3Q0)"
        );
    }
    effective
}

async fn apply_required_runtime_mode(
    agent: &AgentInstance,
    backend: Option<&str>,
    mode: &str,
) -> Result<RequiredFullAutoApplication, AgentError> {
    // a-pure (ELECTRON-3RQ): for metadata-bearing ACP agents (Kimi et al.),
    // resolve cron's full-auto request to the backend-native YOLO id and apply
    // it look-before-leap. This logic is sunk into dream-ai-agent because the
    // `yolo_id` it needs is only visible on the ACP manager's metadata.
    if let Some(app) = agent.apply_required_full_auto_mode().await? {
        return Ok(app);
    }
    // Retained legacy path for non-ACP variants (claude/codex Session, dream):
    // codex ELECTRON-3Q0 catalog alignment + native pass-through.
    let effective = resolve_required_runtime_mode(agent, backend, mode).await;
    agent.set_config_option("mode", &effective).await?;
    Ok(RequiredFullAutoApplication::Applied { effective })
}

fn send_error_display_message(error: &AgentSendError) -> String {
    error
        .stream_error()
        .detail
        .clone()
        .unwrap_or_else(|| error.stream_error().message.clone())
}

fn turn_attempt_error_message(summary: &TurnAttemptSummary) -> Option<String> {
    summary.terminal_error.as_ref().map(|error| {
        error
            .detail
            .as_deref()
            .filter(|detail| !detail.trim().is_empty())
            .unwrap_or(error.message.as_str())
            .to_owned()
    })
}

async fn record_agent_session_failure(
    service: &ConversationService,
    user_id: &str,
    agent_id: Option<&str>,
    code: &str,
    message: &str,
) {
    let Some(agent_id) = agent_id else {
        return;
    };
    let Some(feedback) = service.agent_availability_feedback() else {
        return;
    };
    if let Err(error) = feedback.record_session_failure(user_id, agent_id, code, message).await {
        warn!(
            user_id,
            agent_id,
            code,
            error = %ErrorChain(&error),
            "Failed to record agent availability session failure"
        );
    }
}

async fn record_agent_session_success(service: &ConversationService, user_id: &str, agent_id: Option<&str>) {
    let Some(agent_id) = agent_id else {
        return;
    };
    let Some(feedback) = service.agent_availability_feedback() else {
        return;
    };
    if let Err(error) = feedback.record_session_success(user_id, agent_id).await {
        warn!(
            user_id,
            agent_id,
            error = %ErrorChain(&error),
            "Failed to record agent availability session success"
        );
    }
}

/// Record everything one completed attempt cost.
///
/// Two kinds of usage, deliberately kept as separate rows:
///
/// - the turn's own model, from the backend's terminal event;
/// - one row per model a *tool* borrowed mid-turn (today `ReadImage`'s vision
///   delegate). Each is billed separately by the provider and priced by ITS OWN
///   model name — the rate table matches on model — so folding them into the
///   turn would charge the session model for tokens it never spent. Until this
///   existed the delegate's cost was invisible to the company's spend cap.
///
/// Extracted from the orchestrator loop so it can be tested without standing up
/// a whole conversation service.
///
/// `channel_id` (the main turn's raw `providers.id`) is applied ONLY to the
/// turn's own row. The delegate rows get `None` on purpose:
/// `DelegateUsageEventData` carries just `{model, input_tokens, output_tokens}`
/// — no provider id — so attributing a tool's borrowed model call to the main
/// turn's channel would fabricate attribution the channel report would present
/// as fact. Honest gap, not an oversight: fixing it needs the delegate event to
/// start carrying the channel it actually ran on.
fn meter_attempt(
    recorder: &dyn crate::state::UsageRecorder,
    user_id: &str,
    conv_id: &str,
    channel_id: Option<&str>,
    outcome: &RelayOutcome,
) {
    recorder.record_turn(
        user_id.to_owned(),
        conv_id.to_owned(),
        outcome.model.clone(),
        channel_id.map(str::to_owned),
        outcome.input_tokens,
        outcome.output_tokens,
    );
    for delegate in &outcome.delegate_usage {
        recorder.record_turn(
            user_id.to_owned(),
            conv_id.to_owned(),
            Some(delegate.model.clone()),
            // See the doc comment: no channel attribution for delegates.
            None,
            Some(delegate.input_tokens),
            Some(delegate.output_tokens),
        );
    }
}

/// One trace row for the attempt itself plus one per tool delegate — the
/// per-call counterpart of [`meter_attempt`]. A failed attempt is still a
/// row (`error` set): a retry storm is exactly what this trace exists to
/// expose, and folding failures away would hide it.
///
/// `duration_ms` is the attempt's wall-clock time (P1-3), applied ONLY to the
/// attempt's own row. Delegate rows get `None` on purpose:
/// `DelegateUsageEventData` carries no timing, so no honest duration exists
/// for a borrowed model call — inheriting the attempt's would fabricate the
/// latency distribution the enterprise report's P50/P95 is computed from.
/// (Same honesty rule as the delegate's `channel_id = None` above.)
fn trace_attempt(
    recorder: &dyn crate::state::LlmCallTraceRecorder,
    user_id: &str,
    conv_id: &str,
    outcome: &RelayOutcome,
    duration_ms: Option<i64>,
) {
    recorder.record_call(
        user_id.to_owned(),
        conv_id.to_owned(),
        crate::state::LlmCallTrace {
            model: outcome.model.clone(),
            input_tokens: outcome.input_tokens,
            output_tokens: outcome.output_tokens,
            duration_ms,
            error: outcome.attempt.terminal_error.as_ref().map(|e| {
                if e.message.is_empty() {
                    "agent stream ended with an error".to_owned()
                } else {
                    e.message.clone()
                }
            }),
        },
    );
    for delegate in &outcome.delegate_usage {
        recorder.record_call(
            user_id.to_owned(),
            conv_id.to_owned(),
            crate::state::LlmCallTrace {
                model: Some(delegate.model.clone()),
                input_tokens: Some(delegate.input_tokens),
                output_tokens: Some(delegate.output_tokens),
                // See the doc comment: no independent timing exists for a
                // delegate call, so it stays None rather than a guess.
                duration_ms: None,
                error: None,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_ai_agent::protocol::events::DelegateUsageEventData;

    use crate::stream_relay::RelayTerminal;

    /// Captures what the per-call trace plane would have been told (P2-5,
    /// P1-3 added the duration slot).
    #[derive(Default)]
    struct RecordingLlmTraceRecorder(
        std::sync::Mutex<
            Vec<(
                String,
                Option<String>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<String>,
            )>,
        >,
    );

    impl crate::state::LlmCallTraceRecorder for RecordingLlmTraceRecorder {
        fn record_call(&self, user_id: String, _conversation_id: String, trace: crate::state::LlmCallTrace) {
            self.0.lock().unwrap().push((
                user_id,
                trace.model,
                trace.input_tokens,
                trace.output_tokens,
                trace.duration_ms,
                trace.error,
            ));
        }
    }

    /// Captures what the billing plane would have been told.
    #[derive(Default)]
    struct RecordingUsageRecorder(
        std::sync::Mutex<Vec<(String, Option<String>, Option<String>, Option<i64>, Option<i64>)>>,
    );

    impl crate::state::UsageRecorder for RecordingUsageRecorder {
        fn record_turn(
            &self,
            user_id: String,
            _conversation_id: String,
            model: Option<String>,
            channel_id: Option<String>,
            input_tokens: Option<i64>,
            output_tokens: Option<i64>,
        ) {
            self.0
                .lock()
                .unwrap()
                .push((user_id, model, channel_id, input_tokens, output_tokens));
        }
    }

    /// The governance assertion for the vision delegate's cost: a turn where a
    /// tool borrowed another model must produce TWO usage rows, priced under
    /// two different model names. One merged row would charge the session
    /// model for tokens it never spent — and the rate table matches on model
    /// name, so the delegate's real rate would never be applied.
    #[test]
    fn a_delegated_model_call_is_metered_as_its_own_row() {
        let recorder = RecordingUsageRecorder::default();
        let outcome = RelayOutcome {
            model: Some("deepseek-v4-flash".into()),
            input_tokens: Some(900),
            output_tokens: Some(80),
            delegate_usage: vec![DelegateUsageEventData {
                model: "kimi-k2-6".into(),
                input_tokens: 1_234,
                output_tokens: 56,
            }],
            ..finish_outcome(false)
        };

        meter_attempt(&recorder, "user-1", "conv-1", None, &outcome);

        let rows = recorder.0.lock().unwrap();
        assert_eq!(rows.len(), 2, "the delegate call needs a row of its own");
        assert_eq!(
            rows[0],
            (
                "user-1".into(),
                Some("deepseek-v4-flash".into()),
                None,
                Some(900),
                Some(80)
            )
        );
        assert_eq!(
            rows[1],
            ("user-1".into(), Some("kimi-k2-6".into()), None, Some(1_234), Some(56)),
            "the delegate row must be priced under the model that was actually called"
        );
    }

    #[test]
    fn every_delegate_call_gets_its_own_row() {
        let recorder = RecordingUsageRecorder::default();
        let outcome = RelayOutcome {
            delegate_usage: (1..=3)
                .map(|n| DelegateUsageEventData {
                    model: "kimi-k2-6".into(),
                    input_tokens: n * 10,
                    output_tokens: n,
                })
                .collect(),
            ..finish_outcome(false)
        };

        meter_attempt(&recorder, "user-1", "conv-1", None, &outcome);

        assert_eq!(recorder.0.lock().unwrap().len(), 4, "1 turn + 3 delegate calls");
    }

    /// The common case must be unchanged: exactly one row, no phantom extras.
    #[test]
    fn a_turn_without_a_delegate_call_records_one_row() {
        let recorder = RecordingUsageRecorder::default();

        meter_attempt(&recorder, "user-1", "conv-1", None, &finish_outcome(false));

        assert_eq!(recorder.0.lock().unwrap().len(), 1);
    }

    /// The main turn's row carries the channel it ran on; a delegate's row
    /// carries NONE. The delegate event has no provider id, so inheriting the
    /// main turn's channel would fabricate attribution in the channel report.
    #[test]
    fn the_main_turn_is_attributed_to_its_channel_but_delegates_are_not() {
        let recorder = RecordingUsageRecorder::default();
        let outcome = RelayOutcome {
            model: Some("deepseek-v4-flash".into()),
            input_tokens: Some(900),
            output_tokens: Some(80),
            delegate_usage: vec![DelegateUsageEventData {
                model: "kimi-k2-6".into(),
                input_tokens: 10,
                output_tokens: 5,
            }],
            ..finish_outcome(false)
        };

        meter_attempt(&recorder, "user-1", "conv-1", Some("prov_chan_chanA"), &outcome);

        let rows = recorder.0.lock().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].2.as_deref(),
            Some("prov_chan_chanA"),
            "main turn keeps its channel"
        );
        assert_eq!(rows[1].2, None, "delegate must NOT inherit the main turn's channel");
    }

    /// A personal config with a provider id gets that id verbatim; a blank
    /// provider id means no attribution (NULL → `unknown` bucket), never an
    /// empty-string channel key that would render as a blank column entry.
    #[test]
    fn channel_id_from_build_options_takes_the_provider_id_and_rejects_blank() {
        use dream_core_ai_agent::session_context::{
            AcpSessionBuildContext, AgentSessionContext, ConversationContext, WorkspaceContext,
        };
        use dream_core_common::ProviderWithModel;

        let context_for = |provider_id: &str| {
            BuildTaskOptions::new(AgentSessionContext {
                conversation: ConversationContext {
                    conversation_id: "conv-1".into(),
                    user_id: "user-1".into(),
                    agent_type: AgentType::Acp,
                    source: None,
                },
                workspace: WorkspaceContext {
                    path: "/tmp/workspace".into(),
                    stored_path: "/tmp/workspace".into(),
                    is_custom: false,
                },
                model: ProviderWithModel {
                    provider_id: provider_id.to_owned(),
                    model: "model".into(),
                    use_model: None,
                },
                skills: vec![],
                runtime_env: vec![],
                team: None,
                kind: AgentSessionKind::Acp(Box::new(AcpSessionBuildContext {
                    config: Default::default(),
                    team: None,
                    belongs_to_team: false,
                    session_id: None,
                    session_snapshot: None,
                })),
            })
        };

        let resolved = channel_id_from_build_options(&context_for("prov_chan_chanA")).unwrap();
        assert_eq!(resolved, "prov_chan_chanA");

        // Whitespace-only is as good as absent.
        assert_eq!(channel_id_from_build_options(&context_for("   ")), None);
    }

    /// P2-2 memory injection helper: hits are prepended to the kind's preset
    /// field, an existing value is kept after a blank line, and DreamEngine
    /// uses `preset_rules` not `preset_context`.
    #[test]
    fn prepend_preset_context_targets_the_right_field_per_kind() {
        use dream_core_ai_agent::session_context::{
            AcpSessionBuildContext, AgentSessionContext, AionrsSessionBuildContext, ConversationContext,
            WorkspaceContext,
        };
        use dream_core_common::ProviderWithModel;

        let base = |kind| AgentSessionContext {
            conversation: ConversationContext {
                conversation_id: "c".into(),
                user_id: "u".into(),
                agent_type: AgentType::Acp,
                source: None,
            },
            workspace: WorkspaceContext {
                path: "/w".into(),
                stored_path: "/w".into(),
                is_custom: false,
            },
            model: ProviderWithModel {
                provider_id: "p".into(),
                model: "m".into(),
                use_model: None,
            },
            skills: vec![],
            runtime_env: vec![],
            team: None,
            kind,
        };

        // ACP with an existing preset — memory goes first, old value kept.
        let mut acp = base(AgentSessionKind::Acp(Box::new(AcpSessionBuildContext {
            config: dream_core_api_types::AcpBuildExtra {
                preset_context: Some("house rules".into()),
                ..Default::default()
            },
            team: None,
            belongs_to_team: false,
            session_id: None,
            session_snapshot: None,
        })));
        prepend_preset_context(&mut acp, "[Relevant Memory]\nuses pnpm\n[/Relevant Memory]");
        let AgentSessionKind::Acp(c) = &acp.kind else {
            unreachable!()
        };
        let ctx = c.config.preset_context.as_deref().unwrap();
        assert!(ctx.starts_with("[Relevant Memory]"));
        assert!(ctx.ends_with("house rules"));

        // DreamEngine writes preset_rules, not preset_context.
        let mut de = base(AgentSessionKind::DreamEngine(Box::new(AionrsSessionBuildContext {
            config: Default::default(),
            team: None,
            belongs_to_team: false,
        })));
        prepend_preset_context(&mut de, "mem");
        let AgentSessionKind::DreamEngine(c) = &de.kind else {
            unreachable!()
        };
        assert_eq!(c.config.preset_rules.as_deref(), Some("mem"));
    }

    fn finish_outcome(needs_auth: bool) -> RelayOutcome {
        RelayOutcome {
            system_responses: vec![],
            terminal: RelayTerminal::Finish,
            attempt: TurnAttemptSummary {
                needs_auth,
                ..Default::default()
            },
            model: None,
            input_tokens: None,
            output_tokens: None,
            delegate_usage: Vec::new(),
        }
    }

    fn error_outcome(code: AgentErrorCode) -> RelayOutcome {
        RelayOutcome {
            system_responses: vec![],
            terminal: RelayTerminal::Error {
                code: Some(code),
                retryable: None,
            },
            attempt: TurnAttemptSummary::default(),
            model: None,
            input_tokens: None,
            output_tokens: None,
            delegate_usage: Vec::new(),
        }
    }

    // ELECTRON-3Q0: cron forces codex's canonical full-access id (`agent-full-access`,
    // `full_auto_mode_id`) but codex's LIVE catalog advertises the legacy bare token
    // (`full-access`). The direct-CLI turn-time apply must align the two, mirroring
    // AcpAgentManager reconcile — otherwise `set_config_option` REJECTs
    // ("mode 'agent-full-access' is not one of the available modes") and every cron
    // turn fails before the send.
    #[test]
    fn codex_canonical_full_access_downgrades_to_legacy_when_catalog_lacks_canonical() {
        let ids = vec!["auto".to_string(), "full-access".to_string(), "read-only".to_string()];
        assert_eq!(
            normalize_required_mode_for_catalog(Some("codex"), "agent-full-access", &ids),
            "full-access",
            "canonical id absent but legacy present → downgrade so set_config_option accepts"
        );
    }

    #[test]
    fn codex_canonical_full_access_kept_when_catalog_has_it() {
        let ids = vec!["auto".to_string(), "agent-full-access".to_string()];
        assert_eq!(
            normalize_required_mode_for_catalog(Some("codex"), "agent-full-access", &ids),
            "agent-full-access",
            "a live catalog that carries the canonical id keeps it verbatim"
        );
    }

    #[test]
    fn codex_full_access_left_for_local_reject_when_no_full_access_tier() {
        // No full-access tier at all → leave the value so set_config_option still
        // REJECTs; never silently pick a weaker mode.
        let ids = vec!["auto".to_string(), "read-only".to_string()];
        assert_eq!(
            normalize_required_mode_for_catalog(Some("codex"), "agent-full-access", &ids),
            "agent-full-access"
        );
    }

    #[test]
    fn non_full_access_required_mode_passes_through() {
        let ids = vec!["auto".to_string(), "full-access".to_string()];
        assert_eq!(
            normalize_required_mode_for_catalog(Some("codex"), "auto", &ids),
            "auto",
            "only the canonical full-access id is normalized; other required modes pass through"
        );
    }

    #[test]
    fn non_codex_backend_never_normalized() {
        let ids = vec!["full-access".to_string()];
        assert_eq!(
            normalize_required_mode_for_catalog(Some("claude"), "agent-full-access", &ids),
            "agent-full-access",
            "the codex canonical/legacy mapping must not touch other backends"
        );
    }

    #[test]
    fn empty_catalog_leaves_mode_unchanged() {
        // An empty / not-yet-discovered catalog is permissive at set_config_option,
        // so leave the requested value untouched here.
        assert_eq!(
            normalize_required_mode_for_catalog(Some("codex"), "agent-full-access", &[]),
            "agent-full-access"
        );
    }

    #[test]
    fn needs_auth_empty_turn_is_auth_failure() {
        assert!(terminal_is_auth_failure(&finish_outcome(true)));
    }

    #[test]
    fn plain_finish_is_not_auth_failure() {
        // A generic empty turn (or any normal finish) must NOT flip availability.
        assert!(!terminal_is_auth_failure(&finish_outcome(false)));
    }

    #[test]
    fn explicit_auth_error_codes_are_auth_failure() {
        assert!(terminal_is_auth_failure(&error_outcome(
            AgentErrorCode::UserAgentAuthRequired
        )));
        assert!(terminal_is_auth_failure(&error_outcome(
            AgentErrorCode::UserLlmProviderAuthFailed
        )));
        assert!(terminal_is_auth_failure(&error_outcome(
            AgentErrorCode::UserLlmProviderAwsSsoExpired
        )));
    }

    #[test]
    fn non_auth_errors_are_not_auth_failure() {
        assert!(!terminal_is_auth_failure(&error_outcome(
            AgentErrorCode::UnknownUpstreamError
        )));
        assert!(!terminal_is_auth_failure(&error_outcome(
            AgentErrorCode::UserLlmProviderRateLimited
        )));
        assert!(!terminal_is_auth_failure(&error_outcome(
            AgentErrorCode::UserLlmProviderBillingRequired
        )));
    }
    #[test]
    fn a_failed_attempt_is_traced_with_its_error_not_folded_away() {
        let recorder = RecordingLlmTraceRecorder::default();
        let mut outcome = finish_outcome(true);
        outcome.attempt.terminal_error = Some(dream_core_ai_agent::protocol::events::ErrorEventData::legacy(
            "upstream 429",
            None,
        ));
        outcome.model = Some("deepseek-v4-flash".into());
        outcome.input_tokens = Some(50);
        outcome.output_tokens = Some(0);

        trace_attempt(&recorder, "user-1", "conv-1", &outcome, Some(1500));

        let rows = recorder.0.lock().unwrap().clone();
        assert_eq!(rows.len(), 1, "the failure is one trace row, not zero: {rows:?}");
        assert_eq!(rows[0].1.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(rows[0].4, Some(1500), "the attempt's wall-clock lands on its own row");
        assert_eq!(rows[0].5.as_deref(), Some("upstream 429"));
    }

    #[test]
    fn a_delegated_model_call_is_traced_as_its_own_row_too() {
        let recorder = RecordingLlmTraceRecorder::default();
        let outcome = RelayOutcome {
            model: Some("deepseek-v4-flash".into()),
            input_tokens: Some(900),
            output_tokens: Some(80),
            delegate_usage: vec![DelegateUsageEventData {
                model: "kimi-k2-6".into(),
                input_tokens: 1200,
                output_tokens: 40,
            }],
            ..finish_outcome(true)
        };

        trace_attempt(&recorder, "user-1", "conv-1", &outcome, Some(1500));

        let rows = recorder.0.lock().unwrap().clone();
        assert_eq!(rows.len(), 2, "attempt + delegate: {rows:?}");
        assert_eq!(rows[0].1.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(rows[0].4, Some(1500), "the attempt keeps its own wall-clock");
        assert_eq!(rows[1].1.as_deref(), Some("kimi-k2-6"));
        assert_eq!(rows[1].2, Some(1200));
        assert_eq!(
            rows[1].4, None,
            "a delegate has no independent timer — it must stay None, not inherit the attempt's duration"
        );
    }

    /// P1-3: a duration the caller could not measure honestly (`None`) stays
    /// `None` on the row — no zero-substitution, which would drag the latency
    /// percentiles toward 0ms and make the report lie.
    #[test]
    fn an_unmeasured_attempt_traces_null_duration() {
        let recorder = RecordingLlmTraceRecorder::default();

        trace_attempt(&recorder, "user-1", "conv-1", &finish_outcome(false), None);

        let rows = recorder.0.lock().unwrap().clone();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].4, None);
    }
}
