//! Per-turn enterprise memory recall (P2-2, §B.4 完整版).
//!
//! The trait lives in the capability layer because `dream-core-ai-agent`
//! cannot depend on `dream-domain-memory` (domain layer); the implementation
//! is `dream_domain_memory`-backed and wired in dream-app. `None` in personal
//! builds — nothing registered, prompts flow through unmodified.
//!
//! Both conversation paths use this: the ACP prompt pipeline through
//! `manager::acp::memory_prompt_hook`, and the dream engine directly in
//! `send_message`. They share [`recall_prefix`] rather than each formatting
//! their own block, because a member who switches backends must not see the
//! company's memory arrive in a different shape.

/// Recalls the caller's readable enterprise memory for injection into an
/// agent prompt. Unlike the first-turn `preset_context` path, a hook
/// registered in the ACP prompt pipeline runs on EVERY turn — accumulated
/// memory reaches continuing conversations, not just new ones.
///
/// Implementations MUST be fast (one indexed, tokenised query) and MUST
/// degrade to an empty Vec on any error: a memory lookup that fails must
/// never delay or fail a turn.
#[async_trait::async_trait]
pub trait TurnMemoryRecall: Send + Sync {
    async fn recall(&self, user_id: &str, query: &str) -> Vec<String>;
}

/// How long a turn will wait for recall before going without it.
const RECALL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);

/// Bound on the text used as the query. The whole prompt is fed in so the
/// caller stays agnostic about which part carries intent.
const QUERY_MAX_CHARS: usize = 2000;

/// What to put in front of a prompt, and whether asking cost us the turn's
/// patience.
pub enum MemoryPrefix {
    /// Nothing matched, or nothing is stored.
    None,
    /// Prepend this verbatim; it already ends with the blank line.
    Text(String),
    /// Recall did not answer in time. Callers that can warn, should.
    TimedOut,
}

/// Resolve the memory block for one outgoing prompt.
pub async fn recall_prefix(recall: &dyn TurnMemoryRecall, user_id: &str, prompt: &str) -> MemoryPrefix {
    let query: String = prompt.chars().take(QUERY_MAX_CHARS).collect();
    match tokio::time::timeout(RECALL_TIMEOUT, recall.recall(user_id, &query)).await {
        Ok(hits) if !hits.is_empty() => MemoryPrefix::Text(format!(
            "[Relevant Memory]
{}
[/Relevant Memory]

",
            hits.join(
                "
"
            )
        )),
        Ok(_) => MemoryPrefix::None,
        Err(_) => MemoryPrefix::TimedOut,
    }
}

#[cfg(test)]
#[path = "memory_recall_test.rs"]
mod memory_recall_test;
