//! ACP pre-send hook: per-turn enterprise memory injection (P2-2 §B.4 完整版).
//!
//! Registered only when the app wired a recall implementation; prepends the
//! caller's readable memory to EVERY outgoing prompt so accumulated memory
//! reaches continuing conversations, not just the first turn.

use std::sync::Arc;

use crate::capability::memory_recall::TurnMemoryRecall;
use crate::capability::prompt_pipeline::{PreSendHook, PromptCtx};

/// ACP pre-send hook that prepends recalled memory to every prompt.
///
/// The query is the outgoing prompt text itself — the implementation
/// tokenises it; feeding the whole prompt (bounded) keeps the hook
/// content-agnostic about what part of the message carries intent.
pub struct MemoryPromptHook {
    pub recall: Arc<dyn TurnMemoryRecall>,
}

const MEMORY_HOOK_NAME: &str = "memory_prompt_recall";
const RECALL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);
const QUERY_MAX_CHARS: usize = 2000;

#[async_trait::async_trait]
impl PreSendHook for MemoryPromptHook {
    async fn pre_send(&self, ctx: &mut PromptCtx<'_>, prompt: String) -> String {
        let query: String = prompt.chars().take(QUERY_MAX_CHARS).collect();
        let recalled = tokio::time::timeout(
            RECALL_TIMEOUT,
            self.recall.recall(&ctx.params.user_id, &query),
        )
        .await;
        match recalled {
            Ok(hits) if !hits.is_empty() => {
                format!("[Relevant Memory]
{}
[/Relevant Memory]

{prompt}", hits.join("
"))
            }
            Ok(_) => prompt,
            Err(_) => {
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
