//! Executable expert dependencies carried only by an authorized employee bundle.
use crate::{AssistantDefaultScalarResponse, AssistantProfileResponse, AssistantPromptsResponse};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamAssistantSnapshot {
    /// Opaque source identity, never a local assistant id.
    pub key: String,
    pub profile: AssistantProfileResponse,
    pub backend: String,
    pub rules: String,
    pub prompts: AssistantPromptsResponse,
    pub permission: AssistantDefaultScalarResponse,
    pub thought_level: AssistantDefaultScalarResponse,
}
