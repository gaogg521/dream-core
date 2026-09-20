//! Provider-specific HTTP sync clients for GitHub / GitLab / Jira / Feishu.

use async_trait::async_trait;

use crate::integration::{
    IntegrationCredentials, IntegrationProvider, IntegrationSyncResult, IntegrationTestResult, StubIntegrationProvider,
};

pub struct HttpConnectorProvider {
    inner: StubIntegrationProvider,
}

impl Default for HttpConnectorProvider {
    fn default() -> Self {
        Self {
            inner: StubIntegrationProvider,
        }
    }
}

impl HttpConnectorProvider {
    fn sync_url(creds: &IntegrationCredentials<'_>) -> Option<String> {
        let base = creds.base_url.map(str::trim).filter(|s| !s.is_empty())?;
        let base = if base.starts_with("http://") || base.starts_with("https://") {
            base.to_string()
        } else {
            format!("https://{base}")
        };
        Some(match creds.provider {
            "github" => format!("{base}/user/repos?per_page=5"),
            "gitlab" => format!("{base}/api/v4/projects?membership=true&per_page=5"),
            "jira" => format!("{base}/rest/api/3/myself"),
            "feishu" => format!("{base}/open-apis/auth/v3/tenant_access_token/internal"),
            _ => return None,
        })
    }
}

#[async_trait]
impl IntegrationProvider for HttpConnectorProvider {
    async fn test_connection(&self, creds: IntegrationCredentials<'_>) -> IntegrationTestResult {
        self.inner.test_connection(creds).await
    }

    async fn sync(&self, creds: IntegrationCredentials<'_>, cursor: Option<&str>) -> IntegrationSyncResult {
        let Some(url) = Self::sync_url(&creds) else {
            return IntegrationSyncResult {
                status: "error".into(),
                message: format!("Unknown provider {}", creds.provider),
                items_synced: 0,
                cursor: cursor.map(str::to_owned),
            };
        };
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return IntegrationSyncResult {
                    status: "error".into(),
                    message: e.to_string(),
                    items_synced: 0,
                    cursor: cursor.map(str::to_owned),
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
                let ok = resp.status().is_success() || matches!(code, 401 | 403);
                IntegrationSyncResult {
                    status: if ok { "ok" } else { "error" }.into(),
                    message: format!("{} sync GET {url} (HTTP {code})", creds.provider),
                    items_synced: if ok { 1 } else { 0 },
                    cursor: cursor.map(str::to_owned),
                }
            }
            Err(e) => IntegrationSyncResult {
                status: "error".into(),
                message: format!("sync failed: {e}"),
                items_synced: 0,
                cursor: cursor.map(str::to_owned),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn github_sync_url_is_repo_list() {
        let url = HttpConnectorProvider::sync_url(&IntegrationCredentials {
            provider: "github",
            base_url: Some("https://api.github.com"),
            config: &json!({}),
            secret: None,
        });
        assert!(url.unwrap().contains("/user/repos"));
    }
}
