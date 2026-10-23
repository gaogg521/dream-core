//! Provider-specific HTTP sync clients for GitHub / GitLab / Jira / Feishu.
//!
//! GitHub, GitLab and Jira are judged on the *credential*, not on whether the
//! host answers: an HTTP 401, a rate-limited 403 or an empty answer to an
//! unauthenticated call is not a working connector, and reporting it as one
//! sends an administrator off to debug something that was never connected.
//! Feishu keeps the older reachability-only check (its token endpoint is a
//! POST, so a bare GET cannot tell a good credential from a bad one).

use async_trait::async_trait;

use crate::integration::{
    IntegrationCredentials, IntegrationProvider, IntegrationSyncResult, IntegrationTestResult, StubIntegrationProvider,
};

const USER_AGENT: &str = "one-work-enterprise-connector";

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

/// What one authenticated call to a provider told us.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Ok {
        items: u32,
    },
    /// The host answered but the credential was not accepted, or it is being
    /// throttled. Carries the reason shown to the administrator.
    Rejected(String),
}

impl HttpConnectorProvider {
    fn sync_url(creds: &IntegrationCredentials<'_>) -> Option<String> {
        let base = creds.base_url.map(str::trim).filter(|s| !s.is_empty())?;
        let base = if base.starts_with("http://") || base.starts_with("https://") {
            base.to_string()
        } else {
            format!("https://{base}")
        };
        let base = base.trim_end_matches('/');
        Some(match creds.provider {
            "github" => format!("{base}/user/repos?per_page=5"),
            "gitlab" => format!("{base}/api/v4/projects?membership=true&per_page=5"),
            "jira" => format!("{base}/rest/api/3/myself"),
            "feishu" => format!("{base}/open-apis/auth/v3/tenant_access_token/internal"),
            _ => return None,
        })
    }

    /// Providers whose verdict depends on the token being accepted.
    fn needs_token(provider: &str) -> bool {
        matches!(provider, "github" | "gitlab" | "jira")
    }

    /// What is still missing before a token connector can be tested at all.
    fn missing_setup(creds: &IntegrationCredentials<'_>) -> Option<&'static str> {
        if creds.base_url.map(str::trim).filter(|s| !s.is_empty()).is_none() {
            return Some("请先保存服务地址，再测试");
        }
        if creds.secret.map(str::trim).filter(|s| !s.is_empty()).is_none() {
            return Some("请先保存访问令牌，再测试");
        }
        None
    }

    /// Turn an HTTP answer into a verdict. Pure, so every branch is testable
    /// without a network.
    fn judge(provider: &str, code: u16, rate_remaining_zero: bool, body: &str) -> Verdict {
        match code {
            200..=299 => {
                let items = serde_json::from_str::<serde_json::Value>(body)
                    .ok()
                    .and_then(|v| v.as_array().map(|a| a.len() as u32))
                    .unwrap_or(1);
                Verdict::Ok { items }
            }
            401 => Verdict::Rejected("令牌无效或已过期（HTTP 401），请重新生成并保存".into()),
            403 if rate_remaining_zero => {
                Verdict::Rejected("请求被限流（HTTP 403，额度已用尽），稍后重试，或换用带权限的令牌".into())
            }
            403 => Verdict::Rejected(format!(
                "令牌权限不足或被拒绝（HTTP 403）。请确认令牌具备读取{}的权限",
                match provider {
                    "gitlab" => "项目（read_api）",
                    "jira" => "用户信息",
                    _ => "仓库（repo）",
                }
            )),
            404 => Verdict::Rejected("服务地址不对：接口不存在（HTTP 404），请检查地址是否包含多余路径".into()),
            other => Verdict::Rejected(format!("服务返回 HTTP {other}")),
        }
    }

    async fn call(&self, creds: &IntegrationCredentials<'_>) -> Result<Verdict, String> {
        let Some(url) = Self::sync_url(creds) else {
            return Err(format!("未知的连接器 {}", creds.provider));
        };
        let token = creds.secret.map(str::trim).filter(|s| !s.is_empty());
        let Some(token) = token else {
            return Err("请先保存访问令牌，再测试".into());
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| e.to_string())?;
        let mut request = client.get(&url).bearer_auth(token);
        if creds.provider == "github" {
            request = request.header("Accept", "application/vnd.github+json");
        }
        let response = request.send().await.map_err(|e| format!("无法连接 {url}：{e}"))?;
        let code = response.status().as_u16();
        let rate_remaining_zero = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim() == "0");
        let body = response.text().await.unwrap_or_default();
        Ok(Self::judge(creds.provider, code, rate_remaining_zero, &body))
    }
}

#[async_trait]
impl IntegrationProvider for HttpConnectorProvider {
    async fn test_connection(&self, creds: IntegrationCredentials<'_>) -> IntegrationTestResult {
        if !Self::needs_token(creds.provider) {
            return self.inner.test_connection(creds).await;
        }
        if let Some(missing) = Self::missing_setup(&creds) {
            return IntegrationTestResult {
                status: "not_configured".into(),
                message: missing.into(),
            };
        }
        match self.call(&creds).await {
            Ok(Verdict::Ok { items }) => IntegrationTestResult {
                status: "ok".into(),
                message: format!("连接正常，令牌有效（读取到 {items} 项）"),
            },
            Ok(Verdict::Rejected(reason)) => IntegrationTestResult {
                status: "error".into(),
                message: reason,
            },
            Err(message) => IntegrationTestResult {
                status: "error".into(),
                message,
            },
        }
    }

    async fn sync(&self, creds: IntegrationCredentials<'_>, cursor: Option<&str>) -> IntegrationSyncResult {
        let cursor = cursor.map(str::to_owned);
        if !Self::needs_token(creds.provider) {
            return self.legacy_sync(creds, cursor).await;
        }
        if let Some(missing) = Self::missing_setup(&creds) {
            return IntegrationSyncResult {
                status: "not_configured".into(),
                message: missing.into(),
                items_synced: 0,
                cursor,
            };
        }
        match self.call(&creds).await {
            Ok(Verdict::Ok { items }) => IntegrationSyncResult {
                status: "ok".into(),
                message: format!("{} 同步完成，读取到 {items} 项", creds.provider),
                items_synced: items,
                cursor,
            },
            Ok(Verdict::Rejected(reason)) => IntegrationSyncResult {
                status: "error".into(),
                message: reason,
                items_synced: 0,
                cursor,
            },
            Err(message) => IntegrationSyncResult {
                status: "error".into(),
                message,
                items_synced: 0,
                cursor,
            },
        }
    }
}

impl HttpConnectorProvider {
    /// The reachability-only pull Feishu (and any unknown provider) still uses.
    async fn legacy_sync(&self, creds: IntegrationCredentials<'_>, cursor: Option<String>) -> IntegrationSyncResult {
        let Some(url) = Self::sync_url(&creds) else {
            return IntegrationSyncResult {
                status: "error".into(),
                message: format!("Unknown provider {}", creds.provider),
                items_synced: 0,
                cursor,
            };
        };
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return IntegrationSyncResult {
                    status: "error".into(),
                    message: e.to_string(),
                    items_synced: 0,
                    cursor,
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
                    cursor,
                }
            }
            Err(e) => IntegrationSyncResult {
                status: "error".into(),
                message: format!("sync failed: {e}"),
                items_synced: 0,
                cursor,
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

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        let url = HttpConnectorProvider::sync_url(&IntegrationCredentials {
            provider: "gitlab",
            base_url: Some("https://git.example.com/"),
            config: &json!({}),
            secret: None,
        });
        assert_eq!(
            url.as_deref(),
            Some("https://git.example.com/api/v4/projects?membership=true&per_page=5")
        );
    }

    #[test]
    fn a_two_hundred_counts_the_items_it_returned() {
        assert_eq!(
            HttpConnectorProvider::judge("github", 200, false, r#"[{"id":1},{"id":2},{"id":3}]"#),
            Verdict::Ok { items: 3 }
        );
        assert_eq!(
            HttpConnectorProvider::judge("gitlab", 200, false, "[]"),
            Verdict::Ok { items: 0 }
        );
        // An object body (Jira /myself) is one item.
        assert_eq!(
            HttpConnectorProvider::judge("jira", 200, false, r#"{"accountId":"x"}"#),
            Verdict::Ok { items: 1 }
        );
    }

    #[test]
    fn a_rejected_credential_is_never_reported_as_working() {
        for code in [401, 403, 404, 500] {
            assert!(
                matches!(
                    HttpConnectorProvider::judge("github", code, false, ""),
                    Verdict::Rejected(_)
                ),
                "HTTP {code} must not read as a working connector"
            );
        }
    }

    #[test]
    fn a_throttled_403_is_named_as_throttling() {
        let Verdict::Rejected(reason) = HttpConnectorProvider::judge("github", 403, true, "") else {
            panic!("a 403 is never ok");
        };
        assert!(reason.contains("限流"), "{reason}");
        let Verdict::Rejected(reason) = HttpConnectorProvider::judge("github", 403, false, "") else {
            panic!("a 403 is never ok");
        };
        assert!(reason.contains("权限"), "{reason}");
    }

    #[tokio::test]
    async fn without_a_token_a_token_connector_is_not_configured_rather_than_ok() {
        let provider = HttpConnectorProvider::default();
        let result = provider
            .test_connection(IntegrationCredentials {
                provider: "gitlab",
                base_url: Some("https://git.example.com"),
                config: &json!({}),
                secret: None,
            })
            .await;
        assert_eq!(result.status, "not_configured");
        assert!(result.message.contains("令牌"));
    }

    /// Against a real listener: a server that answers 401 to the token must be
    /// an error, and one that answers 200 with a list must be ok with the count.
    #[tokio::test]
    async fn the_verdict_follows_what_the_server_says_about_the_token() {
        use axum::{Router, http::StatusCode, routing::get};

        async fn serve(router: Router) -> String {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let _ = axum::serve(listener, router).await;
            });
            format!("http://{addr}")
        }

        let rejecting =
            serve(Router::new().route("/api/v4/projects", get(|| async { StatusCode::UNAUTHORIZED }))).await;
        let accepting = serve(Router::new().route(
            "/api/v4/projects",
            get(|| async { axum::Json(json!([{"id": 1}, {"id": 2}])) }),
        ))
        .await;

        let provider = HttpConnectorProvider::default();
        let config = json!({});
        let bad = provider
            .test_connection(IntegrationCredentials {
                provider: "gitlab",
                base_url: Some(&rejecting),
                config: &config,
                secret: Some("glpat-wrong"),
            })
            .await;
        assert_eq!(bad.status, "error");
        assert!(bad.message.contains("401"), "{}", bad.message);

        let good = provider
            .sync(
                IntegrationCredentials {
                    provider: "gitlab",
                    base_url: Some(&accepting),
                    config: &config,
                    secret: Some("glpat-right"),
                },
                None,
            )
            .await;
        assert_eq!(good.status, "ok");
        assert_eq!(good.items_synced, 2);
    }
}
