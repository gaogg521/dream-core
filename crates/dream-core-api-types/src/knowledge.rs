//! Versioned knowledge document editing contracts.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagContentState {
    pub content: String,
    pub revision: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRagContentRequest {
    pub content: String,
    pub expected_revision: i64,
}
