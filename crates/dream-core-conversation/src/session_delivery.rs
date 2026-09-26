//! Cross-conversation message delivery (`@@conv:<id>`) — queue, rate limits, blocks.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dream_core_ai_agent::IWorkerTaskManager;
use dream_core_api_types::TeamSessionBinding;
use dream_core_api_types::{SendMessageRequest, WebSocketMessage};
use dream_core_db::IConversationRepository;
use serde_json::json;
use tracing::{info, warn};

use crate::error::ConversationError;
use crate::markers::{SESSION_MESSAGE_MARKER, SESSIONS_MARKER};
use crate::service::ConversationService;

pub const CONV_TOKEN_PREFIX: &str = "@@conv:";

const OUTBOUND_WINDOW: Duration = Duration::from_secs(60);
const OUTBOUND_MAX: u32 = 20;
const PAIR_WINDOW: Duration = Duration::from_secs(30);
const PAIR_MAX: u32 = 8;
const MAX_DELIVERY_FAILURES: u32 = 12;
pub const CLIENT_PREF_CROSS_SESSION: &str = "crossSessionDelivery";

#[derive(Debug, Clone)]
pub struct PendingDelivery {
    pub from_conversation: String,
    pub to_conversation: String,
    pub user_id: String,
    pub user_body: String,
    pub reply_requested: bool,
    pub failures: u32,
}

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub id: String,
    pub title: String,
    pub workspace: Option<String>,
}

#[async_trait]
pub trait SessionDeliveryGate: Send + Sync {
    /// User + enterprise switches; default implementations return `true`.
    async fn delivery_enabled(&self, user_id: &str) -> bool;
}

pub struct AlwaysOnSessionDeliveryGate;

#[async_trait]
impl SessionDeliveryGate for AlwaysOnSessionDeliveryGate {
    async fn delivery_enabled(&self, _user_id: &str) -> bool {
        true
    }
}

#[derive(Default)]
struct SlidingCounter {
    events: VecDeque<Instant>,
    window: Duration,
    max: u32,
}

impl SlidingCounter {
    fn new(window: Duration, max: u32) -> Self {
        Self {
            events: VecDeque::new(),
            window,
            max,
        }
    }

    fn would_exceed(&mut self) -> bool {
        self.prune();
        self.events.len() as u32 >= self.max
    }

    fn record(&mut self) {
        self.prune();
        self.events.push_back(Instant::now());
    }

    fn prune(&mut self) {
        let cutoff = Instant::now() - self.window;
        while self.events.front().is_some_and(|t| *t < cutoff) {
            self.events.pop_front();
        }
    }
}

#[derive(Default)]
struct RateState {
    outbound: std::collections::HashMap<String, SlidingCounter>,
    pairs: std::collections::HashMap<(String, String), SlidingCounter>,
}

impl RateState {
    fn check_outbound(&mut self, from: &str) -> bool {
        let c = self
            .outbound
            .entry(from.to_owned())
            .or_insert_with(|| SlidingCounter::new(OUTBOUND_WINDOW, OUTBOUND_MAX));
        c.would_exceed()
    }

    fn record_outbound(&mut self, from: &str) {
        self.outbound
            .entry(from.to_owned())
            .or_insert_with(|| SlidingCounter::new(OUTBOUND_WINDOW, OUTBOUND_MAX))
            .record();
    }

    fn check_pair(&mut self, a: &str, b: &str) -> bool {
        let key = pair_key(a, b);
        let c = self
            .pairs
            .entry(key)
            .or_insert_with(|| SlidingCounter::new(PAIR_WINDOW, PAIR_MAX));
        c.would_exceed()
    }

    fn record_pair(&mut self, a: &str, b: &str) {
        let key = pair_key(a, b);
        self.pairs
            .entry(key)
            .or_insert_with(|| SlidingCounter::new(PAIR_WINDOW, PAIR_MAX))
            .record();
    }
}

fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

pub struct SessionDeliveryHub {
    queue: Mutex<Vec<PendingDelivery>>,
    rates: Mutex<RateState>,
    gate: Mutex<Option<std::sync::Arc<dyn SessionDeliveryGate>>>,
}

impl Default for SessionDeliveryHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionDeliveryHub {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(Vec::new()),
            rates: Mutex::new(RateState::default()),
            gate: Mutex::new(Some(std::sync::Arc::new(AlwaysOnSessionDeliveryGate))),
        }
    }

    pub fn set_gate(&self, gate: std::sync::Arc<dyn SessionDeliveryGate>) {
        if let Ok(mut g) = self.gate.lock() {
            *g = Some(gate);
        }
    }

    pub fn pending_len(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn clear_for_conversation(&self, conversation_id: &str) {
        if let Ok(mut q) = self.queue.lock() {
            q.retain(|p| p.from_conversation != conversation_id && p.to_conversation != conversation_id);
        }
    }

    pub fn enqueue(&self, item: PendingDelivery) {
        if let Ok(mut q) = self.queue.lock() {
            q.push(item);
        }
    }

    /// Rate-limit check only — no DB access.
    pub fn rate_limit_would_block(&self, from: &str, to: &str) -> bool {
        let Ok(mut rates) = self.rates.lock() else {
            return true;
        };
        rates.check_outbound(from) || rates.check_pair(from, to)
    }

    pub(crate) fn record_rate(&self, from: &str, to: &str) {
        if let Ok(mut rates) = self.rates.lock() {
            rates.record_outbound(from);
            rates.record_pair(from, to);
        }
    }

    async fn gate_allows(&self, user_id: &str) -> bool {
        let gate = self.gate.lock().ok().and_then(|g| g.clone());
        match gate {
            Some(g) => g.delivery_enabled(user_id).await,
            None => true,
        }
    }

    pub async fn try_apply_outbound_tokens(
        &self,
        user_id: &str,
        from_conversation: &str,
        content: &str,
        reply_requested: bool,
        repo: &dyn IConversationRepository,
    ) -> Result<(String, Vec<PendingDelivery>), ConversationError> {
        let (stripped, target_ids) = extract_conv_tokens(content);
        if target_ids.is_empty() {
            return Ok((content.to_owned(), Vec::new()));
        }

        if !self.gate_allows(user_id).await {
            return Err(ConversationError::PolicyDenied {
                code: "SESSION_DELIVERY_DISABLED",
                message: "Cross-session delivery is disabled".into(),
                details: None,
            });
        }

        for to_id in &target_ids {
            if self.rate_limit_would_block(from_conversation, to_id) {
                return Err(ConversationError::PolicyDenied {
                    code: "SESSION_DELIVERY_RATE_LIMIT",
                    message: "Cross-session delivery rate limit exceeded".into(),
                    details: Some(json!({ "to_conversation_id": to_id })),
                });
            }
        }

        let mut resolved = Vec::new();
        for to_id in &target_ids {
            if to_id == from_conversation {
                return Err(ConversationError::BadRequest {
                    reason: "Cannot deliver a message to the same conversation".into(),
                });
            }
            let row = repo
                .get(user_id, to_id)
                .await?
                .ok_or_else(|| ConversationError::BadRequest {
                    reason: format!("Unknown conversation reference: {to_id}"),
                })?;
            if TeamSessionBinding::team_id_marker_from_extra_str(&row.extra).is_some() {
                return Err(ConversationError::Forbidden {
                    reason: "Cannot deliver to a team-owned conversation".into(),
                });
            }
            resolved.push(ResolvedTarget {
                id: row.id.clone(),
                title: row.name.clone(),
                workspace: workspace_from_extra(&row.extra),
            });
        }

        let from_row = repo
            .get(user_id, from_conversation)
            .await?
            .ok_or_else(|| ConversationError::NotFound {
                id: from_conversation.to_owned(),
            })?;
        let from_ws = workspace_from_extra(&from_row.extra);

        let block = build_sessions_block(&resolved, &stripped, reply_requested, from_ws.as_deref());
        let mut deliveries = Vec::new();
        for target in resolved {
            self.record_rate(from_conversation, &target.id);
            deliveries.push(PendingDelivery {
                from_conversation: from_conversation.to_owned(),
                to_conversation: target.id,
                user_id: user_id.to_owned(),
                user_body: stripped.clone(),
                reply_requested,
                failures: 0,
            });
        }

        let combined = if stripped.trim().is_empty() {
            block
        } else {
            format!("{stripped}\n\n{block}")
        };

        Ok((combined, deliveries))
    }

    pub fn push_deliveries(&self, items: Vec<PendingDelivery>) {
        if let Ok(mut q) = self.queue.lock() {
            q.extend(items);
        }
    }
}

pub fn extract_conv_tokens(content: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(content.len());
    let mut ids = Vec::new();
    let mut rest = content;
    while let Some(idx) = rest.find(CONV_TOKEN_PREFIX) {
        out.push_str(&rest[..idx]);
        rest = &rest[idx..];
        let after = &rest[CONV_TOKEN_PREFIX.len()..];
        let id: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if id.is_empty() {
            out.push_str(CONV_TOKEN_PREFIX);
            rest = &rest[CONV_TOKEN_PREFIX.len()..];
            continue;
        }
        ids.push(id.clone());
        rest = &after[id.len()..];
    }
    out.push_str(rest);
    (out, ids)
}

pub fn workspace_from_extra(extra: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(extra).ok()?;
    value
        .get("workspace")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub fn build_sessions_block(
    targets: &[ResolvedTarget],
    body: &str,
    reply_requested: bool,
    from_workspace: Option<&str>,
) -> String {
    let payload = json!({
        "targets": targets.iter().map(|t| json!({
            "id": t.id,
            "title": t.title,
            "workspace": t.workspace,
        })).collect::<Vec<_>>(),
        "from_workspace": from_workspace,
        "body": body,
        "reply_requested": reply_requested,
    });
    format!("{SESSIONS_MARKER}\n{payload}")
}

pub fn build_session_message_block(
    from_id: &str,
    from_title: &str,
    from_workspace: Option<&str>,
    to_workspace: Option<&str>,
    body: &str,
    reply_requested: bool,
) -> String {
    let same_workspace = match (from_workspace, to_workspace) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
    };
    let mut payload = json!({
        "from_conversation_id": from_id,
        "from_title": from_title,
        "same_workspace": same_workspace,
        "from_workspace": from_workspace,
        "to_workspace": to_workspace,
        "body": body,
        "reply_requested": reply_requested,
    });
    if reply_requested {
        payload["reply_to_conversation_id"] = json!(from_id);
    }
    format!("{SESSION_MESSAGE_MARKER}\n{payload}")
}

impl ConversationService {
    pub fn session_delivery_hub(&self) -> &std::sync::Arc<SessionDeliveryHub> {
        &self.session_delivery
    }

    pub fn with_session_delivery_gate(&self, gate: std::sync::Arc<dyn SessionDeliveryGate>) {
        self.session_delivery.set_gate(gate);
    }

    pub async fn run_session_delivery_tick(&self, task_manager: &std::sync::Arc<dyn IWorkerTaskManager>) {
        let pending: Vec<PendingDelivery> = self
            .session_delivery
            .queue
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default();
        if pending.is_empty() {
            return;
        }

        let mut retained = Vec::new();
        for mut item in pending {
            if self.runtime_state().is_deleting(&item.to_conversation) {
                continue;
            }
            if self.runtime_state().is_claimed(&item.to_conversation)
                || self.runtime_state().is_cancelling(&item.to_conversation)
                || self.runtime_state().is_restarting(&item.to_conversation)
            {
                retained.push(item);
                continue;
            }

            let Ok(Some(to_row)) = self.conversation_repo().get(&item.user_id, &item.to_conversation).await else {
                continue;
            };
            if TeamSessionBinding::team_id_marker_from_extra_str(&to_row.extra).is_some() {
                continue;
            }

            let Ok(Some(from_row)) = self
                .conversation_repo()
                .get(&item.user_id, &item.from_conversation)
                .await
            else {
                continue;
            };

            let content = build_session_message_block(
                &item.from_conversation,
                &from_row.name,
                workspace_from_extra(&from_row.extra).as_deref(),
                workspace_from_extra(&to_row.extra).as_deref(),
                &item.user_body,
                item.reply_requested,
            );

            match self
                .send_message(
                    &item.user_id,
                    &item.to_conversation,
                    SendMessageRequest {
                        content,
                        files: Vec::new(),
                        inject_skills: Vec::new(),
                        hidden: false,
                        reply_requested: false,
                    },
                    task_manager,
                )
                .await
            {
                Ok(resp) => {
                    info!(
                        from = %item.from_conversation,
                        to = %item.to_conversation,
                        msg_id = %resp.msg_id,
                        "session delivery completed"
                    );
                    self.broadcast_session_event(
                        "session.delivery.completed",
                        &item.user_id,
                        &item.from_conversation,
                        &item.to_conversation,
                    );
                    self.broadcast_session_event(
                        "session.delivery.completed",
                        &item.user_id,
                        &item.to_conversation,
                        &item.from_conversation,
                    );
                }
                Err(err) => {
                    item.failures += 1;
                    if item.failures >= MAX_DELIVERY_FAILURES {
                        warn!(
                            from = %item.from_conversation,
                            to = %item.to_conversation,
                            error = %err,
                            "dropping poison session delivery"
                        );
                        self.broadcast_session_event(
                            "session.delivery.failed",
                            &item.user_id,
                            &item.from_conversation,
                            &item.to_conversation,
                        );
                    } else {
                        retained.push(item);
                    }
                }
            }
        }

        if let Ok(mut q) = self.session_delivery.queue.lock() {
            q.extend(retained);
        }
    }

    fn broadcast_session_event(&self, event_type: &str, user_id: &str, conversation_id: &str, peer_id: &str) {
        let payload = json!({
            "user_id": user_id,
            "conversation_id": conversation_id,
            "peer_conversation_id": peer_id,
        });
        self.broadcaster().broadcast(WebSocketMessage::new(event_type, payload));
    }

    pub fn clear_session_delivery_for(&self, conversation_id: &str) {
        self.session_delivery.clear_for_conversation(conversation_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tokens_strips_and_collects() {
        let (text, ids) = extract_conv_tokens("hi @@conv:abc and @@conv:xyz!");
        assert_eq!(text, "hi  and !");
        assert_eq!(ids, vec!["abc", "xyz"]);
    }

    #[test]
    fn rate_limit_blocks_before_db() {
        let hub = SessionDeliveryHub::new();
        let from = "from_conv";
        for i in 0..OUTBOUND_MAX {
            let to = format!("to_{i}");
            assert!(!hub.rate_limit_would_block(from, &to));
            hub.record_rate(from, &to);
        }
        assert!(hub.rate_limit_would_block(from, "to_overflow"));
    }

    #[test]
    fn reply_block_omits_reply_address_when_false() {
        let block = build_session_message_block("a", "Title", Some("/w1"), Some("/w2"), "body", false);
        assert!(!block.contains("reply_to_conversation_id"));
        let block2 = build_session_message_block("a", "Title", Some("/w1"), Some("/w2"), "body", true);
        assert!(block2.contains("reply_to_conversation_id"));
    }

    #[test]
    fn cross_workspace_flag() {
        let block = build_session_message_block("a", "T", Some("/a"), Some("/b"), "x", false);
        assert!(block.contains("\"same_workspace\":false"));
    }

    #[test]
    fn clear_for_conversation_removes_both_directions() {
        // Cancel/delete of X must clear X→A and A→X, keep unrelated B→C —
        // otherwise "stop" would be a lie or would kill unrelated traffic.
        let hub = SessionDeliveryHub::new();
        let item = |from: &str, to: &str| PendingDelivery {
            from_conversation: from.into(),
            to_conversation: to.into(),
            user_id: "user_x".into(),
            user_body: "body".into(),
            reply_requested: false,
            failures: 0,
        };
        hub.enqueue(item("x_conv", "a_conv"));
        hub.enqueue(item("a_conv", "x_conv"));
        hub.enqueue(item("b_conv", "c_conv"));
        hub.clear_for_conversation("x_conv");
        assert_eq!(hub.pending_len(), 1);
    }
}
