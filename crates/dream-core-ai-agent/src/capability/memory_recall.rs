//! Per-turn enterprise memory recall for ACP prompts (P2-2, §B.4 完整版).
//!
//! The trait lives in the capability layer because `dream-core-ai-agent`
//! cannot depend on `dream-domain-memory` (domain layer); the implementation
//! is `dream_domain_memory`-backed and wired in dream-app. `None` in personal
//! builds — no hook registered, prompts flow through unmodified.


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
