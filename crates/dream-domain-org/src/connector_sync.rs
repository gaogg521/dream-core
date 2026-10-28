//! Provider-specific HTTP sync clients for GitHub / GitLab / Jira / Feishu.
//!
//! GitHub, GitLab and Jira are judged on the *credential*, not on whether the
//! host answers: an HTTP 401, a rate-limited 403 or an empty answer to an
//! unauthenticated call is not a working connector, and reporting it as one
//! sends an administrator off to debug something that was never connected.
//! Feishu validates App ID/Secret using its actual POST token protocol.
//! None of these sample reads is a business-resource import.

use async_trait::async_trait;

use crate::integration::{IntegrationCredentials, IntegrationProvider, IntegrationSyncResult, IntegrationTestResult};

const USER_AGENT: &str = "one-work-enterprise-connector";

#[derive(Default)]
pub struct HttpConnectorProvider {
    _private: (),
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
        matches!(provider, "github" | "gitlab" | "jira" | "feishu")
    }

    /// What is still missing before a token connector can be tested at all.
    fn missing_setup(creds: &IntegrationCredentials<'_>) -> Option<&'static str> {
        if creds.base_url.map(str::trim).filter(|s| !s.is_empty()).is_none() {
            return Some("请先保存服务地址，再测试");
        }
        if creds.provider == "feishu"
            && creds
                .config
                .get("appId")
                .and_then(|v| v.as_str())
                .is_none_or(|v| v.trim().is_empty())
        {
            return Some("请在扩展配置中填写 appId，再测试");
        }
        if creds.secret.map(str::trim).filter(|s| !s.is_empty()).is_none() {
            return Some(if creds.provider == "feishu" {
                "请先保存 App Secret，再测试"
            } else {
                "请先保存访问令牌，再测试"
            });
        }
        None
    }

    /// Turn an HTTP answer into a verdict. Pure, so every branch is testable
    /// without a network.
    fn judge(provider: &str, code: u16, rate_remaining_zero: bool, body: &str) -> Verdict {
        if provider == "feishu" {
            if !(200..=299).contains(&code) {
                return Verdict::Rejected(format!("飞书应用凭据校验失败（HTTP {code}）"));
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
                return Verdict::Rejected("飞书返回的凭据校验结果无效".into());
            };
            if value.get("code").and_then(|v| v.as_i64()) == Some(0)
                && value
                    .get("tenant_access_token")
                    .and_then(|v| v.as_str())
                    .is_some_and(|v| !v.trim().is_empty())
                && value.get("expire").and_then(|v| v.as_u64()).is_some_and(|v| v > 0)
            {
                return Verdict::Ok { items: 0 };
            }
            let code = value
                .get("code")
                .and_then(|v| v.as_i64())
                .map_or("未知".into(), |v| v.to_string());
            return Verdict::Rejected(format!(
                "飞书应用凭据被拒绝或响应不完整（错误码 {code}），请核对 App ID、App Secret 及应用状态"
            ));
        }
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
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let mut request = if creds.provider == "feishu" {
            let parsed = reqwest::Url::parse(&url).map_err(|_| "飞书服务地址无效".to_string())?;
            let loopback = parsed.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
            if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
                return Err("飞书应用凭据只允许通过 HTTPS 发送（本机回环测试除外）".into());
            }
            client.post(&url).json(&serde_json::json!({"app_id":creds.config["appId"].as_str().unwrap_or_default().trim(), "app_secret":token}))
        } else {
            client.get(&url).bearer_auth(token)
        };
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
            return IntegrationTestResult {
                status: "not_supported".into(),
                message: "该连接器尚未实现凭据校验".into(),
            };
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
                message: if creds.provider == "feishu" {
                    "飞书应用凭据有效；仅代表令牌获取成功，不代表机器人已连接或业务资源已同步".into()
                } else {
                    format!("连接正常，令牌有效（读取到 {items} 项）")
                },
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
        if creds.provider == "feishu" || !Self::needs_token(creds.provider) {
            return IntegrationSyncResult {
                status: "not_supported".into(),
                message: "该连接器尚未实现业务资源同步；凭据检测请使用测试连接，机器人接入请使用客户端频道".into(),
                items_synced: 0,
                cursor,
            };
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
                message: format!("{} 样本读取完成（{items} 项），尚未导入业务资源", creds.provider),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn feishu_uses_post_credentials_and_does_not_claim_business_sync() {
        use axum::{Json, Router, http::HeaderMap, routing::post};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/open-apis/auth/v3/tenant_access_token/internal",
            post(|headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                assert!(headers.get("authorization").is_none());
                assert_eq!(body["app_id"], "cli_fixture");
                Json(if body["app_secret"] == "correct-secret" {
                    json!({"code":0,"tenant_access_token":"fixture-token","expire":7200})
                } else {
                    json!({"code":10003,"msg":"credentials rejected"})
                })
            }),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = HttpConnectorProvider::default();
        let config = json!({"appId":"cli_fixture"});
        for (secret, expected) in [("correct-secret", "ok"), ("incorrect-secret", "error")] {
            let result = provider
                .test_connection(IntegrationCredentials {
                    provider: "feishu",
                    base_url: Some(&base),
                    config: &config,
                    secret: Some(secret),
                })
                .await;
            assert_eq!(result.status, expected);
            assert!(!result.message.contains(secret));
            assert!(!result.message.contains("fixture-token"));
        }
        let sync = provider
            .sync(
                IntegrationCredentials {
                    provider: "feishu",
                    base_url: Some(&base),
                    config: &config,
                    secret: Some("correct-secret"),
                },
                Some("keep-cursor"),
            )
            .await;
        assert_eq!(sync.status, "not_supported");
        assert_eq!(sync.items_synced, 0);
        assert_eq!(sync.cursor.as_deref(), Some("keep-cursor"));
        task.abort();
    }

    #[test]
    fn feishu_does_not_treat_a_reachable_or_malformed_token_response_as_authenticated() {
        for (status, body) in [
            (200, r#"{"code":10003,"msg":"denied"}"#),
            (200, r#"{"code":0}"#),
            (200, r#"{"code":0,"tenant_access_token":"token","expire":0}"#),
            (200, "not JSON"),
            (401, ""),
            (403, ""),
        ] {
            assert!(matches!(
                HttpConnectorProvider::judge("feishu", status, false, body),
                Verdict::Rejected(_)
            ));
        }
    }

    #[tokio::test]
    async fn feishu_requires_app_id_and_secret_before_sending_credentials() {
        let provider = HttpConnectorProvider::default();
        let no_app = provider
            .test_connection(IntegrationCredentials {
                provider: "feishu",
                base_url: Some("https://open.feishu.cn"),
                config: &json!({}),
                secret: Some("secret"),
            })
            .await;
        assert_eq!(no_app.status, "not_configured");
        assert!(no_app.message.contains("appId"));
        let no_secret = provider
            .test_connection(IntegrationCredentials {
                provider: "feishu",
                base_url: Some("https://open.feishu.cn"),
                config: &json!({"appId":"cli_fixture"}),
                secret: None,
            })
            .await;
        assert_eq!(no_secret.status, "not_configured");
        assert!(no_secret.message.contains("App Secret"));
    }

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
