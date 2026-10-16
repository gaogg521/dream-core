//! Browser contracts for an intranet Office editing session.
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeOfficeAvailability {
    pub enabled: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeOfficeSession {
    pub session_id: String,
    pub script_url: String,
    pub config: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeOfficeSaved {
    pub saved: bool,
    pub revision: u64,
    pub source_version: String,
}

#[derive(Deserialize)]
pub struct KnowledgeOfficeCapability {
    pub token: String,
}

#[derive(Serialize)]
pub struct KnowledgeOfficeCallbackResult {
    pub error: u8,
}
