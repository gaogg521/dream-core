//! Integration connector seam (P2-1).
//!
//! Default provider HTTP-GETs `base_url` (Bearer secret if stored). Empty URL
//! reports `not_configured`.

use std::time::Duration;

use async_trait::async_trait;

/// The connector providers the config layer knows about. Kept as a small,
/// explicit set so the admin UI can enumerate them; the storage layer itself
/// treats `provider` as free text, so adding one here (plus a UI card) is all
/// that a new connector needs before its real sync is built.
pub const KNOWN_PROVIDERS: &[&str] = &["github", "gitlab", "jira", "feishu"];

/// Outcome of a connector "test connection" attempt.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationTestResult {
    /// `"not_configured"`, `"ok"`, or `"error"`.
    pub status: String,
    pub message: String,
}

/// Outcome of a connector sync run (incremental metadata pull).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSyncResult {
    pub status: String,
    pub message: String,
    #[serde(default)]
    pub items_synced: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Non-secret + secret connector configuration handed to a provider for a
/// live operation. `secret` is already decrypted; never log it.
pub struct IntegrationCredentials<'a> {
    pub provider: &'a str,
    pub base_url: Option<&'a str>,
    /// Non-secret provider-specific fields (org / project / repo / board ...).
    pub config: &'a serde_json::Value,
    pub secret: Option<&'a str>,
}

#[async_trait]
pub trait IntegrationProvider: Send + Sync {
    /// Probe whether the saved connector config can reach its external system.
    async fn test_connection(&self, creds: IntegrationCredentials<'_>) -> IntegrationTestResult;

    /// Pull remote metadata (repos, boards, spaces). Default: same reachability
    /// check as `test_connection` with zero items.
    async fn sync(&self, creds: IntegrationCredentials<'_>, cursor: Option<&str>) -> IntegrationSyncResult {
        let probe = self.test_connection(creds).await;
        IntegrationSyncResult {
            status: probe.status,
            message: probe.message,
            items_synced: 0,
            cursor: cursor.map(str::to_owned),
        }
    }
}

/// Default provider: HTTP GET the connector base URL.
pub struct StubIntegrationProvider;

#[async_trait]
impl IntegrationProvider for StubIntegrationProvider {
    async fn sync(&self, creds: IntegrationCredentials<'_>, cursor: Option<&str>) -> IntegrationSyncResult {
        let probe = self.test_connection(creds).await;
        IntegrationSyncResult {
            status: probe.status.clone(),
            message: probe.message,
            items_synced: if probe.status == "ok" { 1 } else { 0 },
            cursor: cursor.map(str::to_owned),
        }
    }

    async fn test_connection(&self, creds: IntegrationCredentials<'_>) -> IntegrationTestResult {
        let Some(raw) = creds.base_url.map(str::trim).filter(|s| !s.is_empty()) else {
            return IntegrationTestResult {
                status: "not_configured".to_owned(),
                message: "Save a base URL for this connector, then test again.".to_owned(),
            };
        };
        let url = if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_owned()
        } else {
            format!("https://{raw}")
        };
        let client = match reqwest::Client::builder().timeout(Duration::from_secs(5)).build() {
            Ok(c) => c,
            Err(e) => {
                return IntegrationTestResult {
                    status: "error".to_owned(),
                    message: e.to_string(),
                };
            }
        };
        let mut req = client.get(&url);
        if let Some(token) = creds.secret.map(str::trim).filter(|s| !s.is_empty()) {
            req = req.bearer_auth(token);
        }
        match req.send().await {
            Ok(resp) => {
                let code = resp.status().as_u16();
                if resp.status().is_success() || matches!(code, 401 | 403 | 404) {
                    IntegrationTestResult {
                        status: "ok".to_owned(),
                        message: format!("{} reached {url} (HTTP {code})", creds.provider),
                    }
                } else {
                    IntegrationTestResult {
                        status: "error".to_owned(),
                        message: format!("{url} returned HTTP {code}"),
                    }
                }
            }
            Err(e) => IntegrationTestResult {
                status: "error".to_owned(),
                message: format!("Could not reach {url}: {e}"),
            },
        }
    }
}
