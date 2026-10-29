//! ACP pre-send hook: per-turn enterprise memory injection (P2-2 §B.4 完整版).
//!
//! Registered only when the app wired a recall implementation; prepends the
//! caller's readable memory to EVERY outgoing prompt so accumulated memory
//! reaches continuing conversations, not just the first turn.
//!
//! The lookup and the block's shape live in
//! [`crate::capability::memory_recall`], shared with the dream engine's own
//! injection point. This file is only the ACP-side wiring and the warning.

use std::sync::Arc;

use crate::capability::memory_recall::{Recalled, TurnMemoryRecall, company_memory_tip, format_prefix, recall_hits};
use crate::capability::prompt_pipeline::{PreSendHook, PromptCtx};

/// ACP pre-send hook that prepends recalled memory to every prompt.
pub struct MemoryPromptHook {
    pub recall: Arc<dyn TurnMemoryRecall>,
}

const MEMORY_HOOK_NAME: &str = "memory_prompt_recall";

#[async_trait::async_trait]
impl PreSendHook for MemoryPromptHook {
    async fn pre_send(&self, ctx: &mut PromptCtx<'_>, prompt: String) -> String {
        match recall_hits(self.recall.as_ref(), &ctx.params.user_id, &prompt).await {
            Recalled::Hits(hits) => {
                let _ = ctx
                    .runtime
                    .event_sender()
                    .send(crate::protocol::events::AgentStreamEvent::Tips(company_memory_tip(
                        &hits,
                    )));
                format!("{}{prompt}", format_prefix(&hits))
            }
            Recalled::None => prompt,
            Recalled::TimedOut => {
                // Half-open relay / slow DB — degrade silently with a warning,
                // matching every other hook's failure contract.
                crate::manager::acp::hooks::emit_hook_warning(
                    ctx,
                    MEMORY_HOOK_NAME,
                    "memory recall timed out; continuing without memory injection",
                );
                prompt
            }
        }
    }
}
