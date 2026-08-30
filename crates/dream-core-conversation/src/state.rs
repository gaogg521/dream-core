use std::sync::Arc;

use crate::service::ConversationService;
use dream_core_ai_agent::{ActiveLeaseRegistry, IWorkerTaskManager};

/// Records one metered turn for the billing/usage plane (P0-3). Fire-and-forget:
/// implementations MUST NOT block or fail the send path — they spawn their own
/// async work. Wired to one-billing in dream-app; `None` in personal builds.
///
/// Called once per completed agent attempt, from `ConversationTurnOrchestrator`
/// — not at accept time. A turn is a real LLM call with a real cost only once
/// it has actually run; `model`/tokens are `None` when the backend never
/// reported usage (currently: ACP-bridged CLIs), in which case a real
/// implementation should record the turn without a cost estimate rather than
/// guessing `$0`.
///
/// `channel_id` is the raw `providers.id` of the configuration the turn ran on
/// (`prov_chan_<channel_id>` for enterprise channels), extracted from the
/// session's `ProviderWithModel` before the agent task consumes it. `None`
/// means the caller has no attribution — tool-delegated model calls today —
/// and implementations must record a NULL, never inherit the main turn's
/// channel: a delegate's borrowed model is not billed to the session's
/// provider, and inventing that attribution would fabricate channel report
/// data. (Same honesty rule as the zero-cost turn: record what is known,
/// surface the gap, do not paper over it.)
pub trait UsageRecorder: Send + Sync {
    fn record_turn(
        &self,
        user_id: String,
        conversation_id: String,
        model: Option<String>,
        channel_id: Option<String>,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
    );
}

/// One completed model call, as the per-call LLM trace (P2-5) records it.
/// Where [`UsageRecorder`] folds an attempt into one billed row, the trace
/// keeps EVERY call — including failed attempts and a tool's delegated
/// vision calls — because "what did the agent actually do, call by call" is
/// the question an administrator is asking when they open it.
pub struct LlmCallTrace {
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// Wall-clock duration of the whole attempt (build + send + stream drain),
    /// in ms — P1-3 latency collection. `None` when the caller has no honest
    /// timer for this call: a tool's delegated model call has no independent
    /// timing today, and fabricating one (e.g. inheriting the attempt's
    /// duration) would poison the P50/P95 the enterprise report computes.
    pub duration_ms: Option<i64>,
    /// `None` = the call succeeded; otherwise the terminal error message.
    pub error: Option<String>,
}

/// Hot-path seam for the per-call trace. Same fire-and-forget contract as
/// [`UsageRecorder`]: implementations spawn their own async work and MUST
/// NOT block or fail the send path. Wired to one-billing in dream-app;
/// `None` in personal builds (no trace rows at all).
pub trait LlmCallTraceRecorder: Send + Sync {
    fn record_call(&self, user_id: String, conversation_id: String, trace: LlmCallTrace);
}

/// One completed conversation turn, handed to the enterprise memory pipeline
/// (P2-2) for salient-fact extraction. Same fire-and-forget contract as
/// [`UsageRecorder`] / [`LlmCallTraceRecorder`]: the implementation spawns
/// its own async work and MUST NOT block or fail the turn. Wired to
/// one-memory in dream-app; `None` in personal builds (no extraction, no
/// rows — bit-for-bit the pre-memory behaviour).
///
/// Fired only for a turn that completed without a terminal failure.
/// `user_message` is the exact text of the turn that just finished; the
/// implementation reads the matching assistant reply from persistence
/// itself. `synthetic_prompt` is true when `user_message` is a
/// cron/automation-built prompt rather than something a human typed — such
/// turns must not be mined for "user facts".
pub trait TurnMemoryExtractor: Send + Sync {
    fn extract_from_turn(&self, user_id: String, conversation_id: String, user_message: String, synthetic_prompt: bool);
}

/// Retrieves the caller's readable enterprise memory (P2-2) for injection
/// into an agent turn's context. `None` in personal builds, exactly like the
/// other seams. Unlike the fire-and-forget recorders this one IS awaited on
/// the turn-start path, so the implementation MUST be fast (one indexed
/// query) and MUST degrade to an empty `Vec` on any error or slowness — a
/// memory lookup that fails or stalls must never delay or block a turn (the
/// caller wraps it in a short timeout regardless).
#[async_trait::async_trait]
pub trait MemoryContextProvider: Send + Sync {
    /// Memory snippets relevant to `query` that `user_id` may read, most
    /// relevant first, already length-bounded. Empty = inject nothing.
    async fn recall(&self, user_id: &str, query: &str) -> Vec<String>;
}

/// Why the product refused an action, in a shape the HTTP layer can hand to a
/// client that wants to say it in the reader's own language.
///
/// Three parts on purpose:
/// - `code` — stable, machine-readable; what a client branches on. Never
///   derived from the message text, which is prose and gets reworded.
/// - `message` — English, for clients with no translation and for the log.
/// - `details` — the parameters a translated sentence needs (which rule, which
///   model). Without them a client can only show the English or nothing.
///
/// ⚠️ Everything in `message` reaches the user. Only put text here that was
/// written for them — never an internal error's `to_string()`.
#[derive(Debug, Clone)]
pub struct PolicyDenial {
    pub code: &'static str,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl PolicyDenial {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

/// Pre-send policy gate (P1-2 model control). Returns `Err(denial)` to BLOCK the
/// send — the team is over its spend budget, or the model is off its allowlist.
/// `None` gate, or an `Ok` result, lets the send proceed. Personal / no-company
/// users always pass. Wired to one-billing in dream-app.
#[async_trait::async_trait]
pub trait SendGate: Send + Sync {
    async fn check_send(&self, user_id: &str, model: Option<&str>) -> Result<(), PolicyDenial>;
    /// Allowlist-only check at model-switch time (budget is enforced at send).
    async fn check_model(&self, user_id: &str, model: &str) -> Result<(), PolicyDenial>;
}

/// Inspects outgoing message text against the company's content rules (T4).
///
/// Returns `Some(reason)` to BLOCK the send. Findings are recorded by the
/// implementation regardless of the return value — a rule set to record-only
/// returns `None` and still leaves a trail. `None` inspector, or a personal
/// build with no rules distributed, always passes. Wired to dream-system in
/// dream-app.
///
/// Synchronous on purpose: the check is an in-memory scan on the hottest path
/// in the product, and making it awaitable would invite an implementation that
/// does I/O there.
pub trait ContentInspector: Send + Sync {
    fn inspect(&self, conversation_id: &str, text: &str) -> Option<PolicyDenial>;
}

/// Shared state for conversation route handlers.
#[derive(Clone)]
pub struct ConversationRouterState {
    pub service: ConversationService,
    pub task_manager: Arc<dyn IWorkerTaskManager>,
    pub active_leases: Arc<ActiveLeaseRegistry>,
    /// Optional pre-send policy gate (P1-2), for the `check_model` allowlist
    /// check ONLY (model-switch time, `routes_aux::set_config_option`). The
    /// `check_send` budget/rate check used to live here too and be checked by
    /// `routes::send_msg`, but T3 sank that one into `ConversationService`
    /// itself (`ConversationService::send_gate`/`with_send_gate`) so it also
    /// covers cron- and IM-channel-triggered turns, which never touch this
    /// router state at all. Model switching is HTTP-only (there is no
    /// "cron/channel switches the model" concept), so this field stays.
    pub send_gate: Option<Arc<dyn SendGate>>,
    /// Optional content inspector (T4); when set, a send may be blocked and
    /// findings recorded.
    pub content_inspector: Option<Arc<dyn ContentInspector>>,
}

impl ConversationRouterState {
    /// Attach a pre-send policy gate (one-billing) for the `check_model`
    /// path only — see the field's doc comment. Chainable at wire-up time.
    pub fn with_send_gate(mut self, gate: Arc<dyn SendGate>) -> Self {
        self.send_gate = Some(gate);
        self
    }

    /// Attach a content inspector (dream-system). Chainable at wire-up time.
    pub fn with_content_inspector(mut self, inspector: Arc<dyn ContentInspector>) -> Self {
        self.content_inspector = Some(inspector);
        self
    }
}
