use serde::{Deserialize, Serialize};

/// Persistent status from the separately privileged deployment update worker.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnterpriseUpdateStatus {
    #[serde(default)]
    pub configured: bool,
    pub phase: String,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub installed_version: Option<String>,
    pub update_available: Option<bool>,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub notes: Option<String>,
    pub message: Option<String>,
    pub error: Option<String>,
    pub rolled_back: Option<bool>,
}
