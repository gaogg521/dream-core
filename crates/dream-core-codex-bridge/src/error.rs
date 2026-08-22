#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("codex bridge is not configured or disabled")]
    NotConfigured,

    #[error("unauthorized")]
    Unauthorized,

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("agent error: {0}")]
    Agent(#[from] dream_core_ai_agent::AgentError),

    #[error("provider error: {0}")]
    Provider(#[from] dream_engine_providers::ProviderError),

    #[error("database error: {0}")]
    Db(#[from] dream_core_db::DbError),
}
