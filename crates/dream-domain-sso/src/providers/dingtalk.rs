//! DingTalk OAuth provider.
//!
//! Translation of `DingTalkAuthProvider.ts`. DingTalk uses a modern
//! userAccessToken endpoint (v1.0) and the legacy gettoken endpoint for
//! credential validation.

use serde::{Deserialize, Serialize};

use crate::error::SsoError;
use crate::providers::ProviderUserInfo;

const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

const AUTHORIZE_URL: &str = "https://login.dingtalk.com/oauth2/auth";
const TOKEN_URL: &str = "https://api.dingtalk.com/v1.0/oauth2/userAccessToken";
const USER_INFO_URL: &str = "https://api.dingtalk.com/v1.0/contact/users/me";
const GETTOKEN_URL: &str = "https://oapi.dingtalk.com/gettoken";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DingtalkProviderConfig {
    pub app_key: String,
    pub app_secret: String,
    #[serde(default)]
    pub corp_id: Option<String>,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default = "default_external_id_field")]
    pub external_id_field: String,
}

fn default_external_id_field() -> String {
    "unionId".into()
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DingtalkUserTokenResponse {
    access_token: Option<String>,
    corp_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DingtalkUserInfo {
    pub union_id: Option<String>,
    pub open_id: Option<String>,
    pub nick: Option<String>,
    pub mobile: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct DingtalkLegacyTokenResponse {
    errcode: Option<i64>,
    errmsg: Option<String>,
    /// Parsed but unused: kept so this struct documents the legacy endpoint's
    /// full response shape next to the fields we do read.
    #[allow(dead_code)]
    access_token: Option<String>,
}

pub struct DingtalkProvider;

impl DingtalkProvider {
    pub fn build_authorize_url(config: &DingtalkProviderConfig, state: &str) -> String {
        let redirect = config.redirect_uri.as_deref().unwrap_or("");
        let scope = if config.corp_id.as_deref().is_some_and(|id| !id.trim().is_empty()) {
            "openid%20corpid%20Contact.User.Read"
        } else {
            "openid%20Contact.User.Read"
        };
        format!(
            "{AUTHORIZE_URL}?redirect_uri={}&response_type=code&client_id={}&scope={scope}&state={}&prompt=consent",
            urlencode(redirect),
            urlencode(&config.app_key),
            urlencode(state),
        )
    }

    pub async fn exchange_code(config: &DingtalkProviderConfig, code: &str) -> Result<String, SsoError> {
        let client = http_client()?;
        let body = serde_json::json!({
            "clientId": config.app_key,
            "clientSecret": config.app_secret,
            "code": code,
            "grantType": "authorization_code",
        });
        let resp = client.post(TOKEN_URL).json(&body).send().await?;
        let status = resp.status();
        let data: DingtalkUserTokenResponse = resp.json().await.unwrap_or_default();
        if !status.is_success() {
            return Err(SsoError::Internal(format!(
                "DingTalk token exchange failed: HTTP {status}"
            )));
        }
        Self::validate_token(config, data)
    }

    fn validate_token(config: &DingtalkProviderConfig, data: DingtalkUserTokenResponse) -> Result<String, SsoError> {
        if let Some(expected) = config.corp_id.as_deref().map(str::trim).filter(|id| !id.is_empty())
            && data.corp_id.as_deref() != Some(expected)
        {
            return Err(SsoError::BadRequest(
                "DingTalk login organization does not match configured CorpId".into(),
            ));
        }
        data.access_token
            .filter(|s| !s.is_empty())
            .ok_or_else(|| SsoError::Internal("DingTalk token exchange: missing access_token".into()))
    }

    pub async fn fetch_user_info(access_token: &str) -> Result<DingtalkUserInfo, SsoError> {
        let client = http_client()?;
        let resp = client
            .get(USER_INFO_URL)
            .header("x-acs-dingtalk-access-token", access_token)
            .send()
            .await?;
        let status = resp.status();
        let data: DingtalkUserInfo = resp.json().await.unwrap_or_default();
        if !status.is_success() {
            return Err(SsoError::Internal(format!("DingTalk user info failed: HTTP {status}")));
        }
        Ok(data)
    }

    pub fn resolve_external_id(info: &DingtalkUserInfo, field: &str) -> Option<String> {
        let (primary, fallback) = if field == "openId" {
            (&info.open_id, &info.union_id)
        } else {
            (&info.union_id, &info.open_id)
        };
        if let Some(v) = primary.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return Some(v.to_owned());
        }
        fallback
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }

    pub fn to_provider_user_info(info: &DingtalkUserInfo, external_id: &str) -> ProviderUserInfo {
        let preferred = info
            .nick
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "dingtalk_{}",
                    crate::providers::synthetic_username_suffix(external_id, 16)
                )
            });
        ProviderUserInfo {
            employee_no: None,
            enterprise_email: None,
            external_id: external_id.to_owned(),
            preferred_username: preferred,
            // NOT `info.mobile`. This field is the person's DEPARTMENT PATH:
            // it is copied onto `one_user_org.org_unit_path` at login and
            // rendered as 部门路径 in the members roster, which every member of
            // the project group can read via `/api/one/org/members`. Putting
            // the mobile number there published everyone's phone number to
            // their colleagues, under a column claiming to be their
            // department.
            //
            // The OAuth contact "me" endpoint returns no department. The
            // separate application directory enrichment fills it when allowed.
            org_unit_path: None,
            job_title: None,
            // DingTalk corp_id is available but not threaded through yet — the
            // auto-join enterprise path currently targets Feishu only.
            org_external_id: None,
        }
    }

    pub async fn test_credentials(app_key: &str, app_secret: &str) -> Result<(), SsoError> {
        let key = app_key.trim();
        let secret = app_secret.trim();
        if key.is_empty() || secret.is_empty() || secret == "******" {
            return Err(SsoError::BadRequest(
                "AppKey and AppSecret are required for connection test".into(),
            ));
        }
        let client = http_client()?;
        let resp = client
            .get(GETTOKEN_URL)
            .query(&[("appkey", key), ("appsecret", secret)])
            .send()
            .await?;
        let status = resp.status();
        let data: DingtalkLegacyTokenResponse = resp.json().await.unwrap_or_default();
        if !status.is_success() {
            return Err(SsoError::Internal(format!("DingTalk API error: HTTP {status}")));
        }
        if data.errcode.unwrap_or(-1) != 0 {
            return Err(SsoError::Internal(
                data.errmsg.unwrap_or_else(|| "DingTalk token request failed".into()),
            ));
        }
        Ok(())
    }

    /// The OAuth `me` endpoint returns a nickname. Resolve its union ID in
    /// the application's enterprise directory before requesting staff details.
    pub async fn enrich_profile(
        config: &DingtalkProviderConfig,
        info: &DingtalkUserInfo,
        profile: &mut ProviderUserInfo,
    ) {
        Self::enrich_profile_from("https://oapi.dingtalk.com", config, info, profile).await;
    }

    async fn enrich_profile_from(
        base: &str,
        config: &DingtalkProviderConfig,
        info: &DingtalkUserInfo,
        profile: &mut ProviderUserInfo,
    ) {
        let result = tokio::time::timeout(HTTP_TIMEOUT, async {
            let union_id = info
                .union_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .ok_or(SsoError::IdentityMissing)?;
            let client = http_client()?;
            let token: serde_json::Value = client
                .get(format!("{base}/gettoken"))
                .query(&[
                    ("appkey", config.app_key.as_str()),
                    ("appsecret", config.app_secret.as_str()),
                ])
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            validate_contact_response(&token)?;
            let token = token["access_token"]
                .as_str()
                .filter(|token| !token.is_empty())
                .ok_or_else(|| SsoError::Internal("DingTalk app token missing".into()))?;
            let identity = contact_request(
                &client,
                base,
                token,
                "user/getbyunionid",
                serde_json::json!({"unionid": union_id}),
            )
            .await?;
            if identity["contact_type"].as_i64() != Some(0) {
                return Err(SsoError::IdentityMissing);
            }
            let user_id = identity["userid"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or(SsoError::IdentityMissing)?;
            let data = contact_request(
                &client,
                base,
                token,
                "v2/user/get",
                serde_json::json!({"userid": user_id}),
            )
            .await?;
            if data["userid"].as_str() != Some(user_id) || data["unionid"].as_str().is_some_and(|id| id != union_id) {
                return Err(SsoError::IdentityMissing);
            }
            apply_contact_profile(profile, &data);
            // Bound parent traversal and reject cycles in provider data.
            let mut id = data["dept_id_list"]
                .as_array()
                .and_then(|ids| ids.first())
                .and_then(|id| id.as_i64())
                .unwrap_or(0);
            let mut visited = std::collections::HashSet::new();
            let mut names = Vec::new();
            while id > 0 && visited.len() < 16 && visited.insert(id) {
                let node = contact_request(
                    &client,
                    base,
                    token,
                    "v2/department/get",
                    serde_json::json!({"dept_id": id}),
                )
                .await?;
                if let Some(name) = node["name"].as_str().map(str::trim).filter(|name| !name.is_empty()) {
                    names.push(name.to_owned());
                }
                id = node["parent_id"].as_i64().unwrap_or(0);
            }
            names.reverse();
            if !names.is_empty() {
                profile.org_unit_path = Some(names.join(" / "));
            }
            Ok::<(), SsoError>(())
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            tracing::warn!(
                provider = "dingtalk",
                "SSO contact profile incomplete; check application Contact permissions and visible member scope"
            );
        }
    }
}

fn validate_contact_response(data: &serde_json::Value) -> Result<(), SsoError> {
    if data["errcode"].as_i64() != Some(0) {
        return Err(SsoError::Internal(format!(
            "DingTalk Contact API failed (code {:?})",
            data["errcode"].as_i64()
        )));
    }
    Ok(())
}

async fn contact_request(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    endpoint: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, SsoError> {
    let data: serde_json::Value = client
        .post(format!("{base}/topapi/{endpoint}"))
        .query(&[("access_token", token)])
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    validate_contact_response(&data)?;
    Ok(data["result"].clone())
}

fn apply_contact_profile(profile: &mut ProviderUserInfo, data: &serde_json::Value) {
    profile.enterprise_email = data["org_email"]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    profile.employee_no = data["job_number"]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    if let Some(name) = data["name"].as_str().map(str::trim).filter(|name| !name.is_empty()) {
        profile.preferred_username = name.to_owned();
    }
    profile.job_title = data["title"]
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned);
}

fn http_client() -> Result<reqwest::Client, SsoError> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| SsoError::Internal(format!("http client: {e}")))
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn contact_http_chain_and_permission_denied_fallback() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_json, method, path, query_param},
        };
        let server = MockServer::start().await;
        let cfg = DingtalkProviderConfig {
            app_key: "app".into(),
            app_secret: "secret".into(),
            corp_id: None,
            redirect_uri: None,
            external_id_field: "unionId".into(),
        };
        let info: DingtalkUserInfo =
            serde_json::from_value(serde_json::json!({"unionId":"union-1","nick":"昵称"})).unwrap();
        Mock::given(method("GET"))
            .and(path("/gettoken"))
            .and(query_param("appkey", "app"))
            .and(query_param("appsecret", "secret"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"errcode":0,"access_token":"app-token"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/topapi/user/getbyunionid"))
            .and(query_param("access_token", "app-token"))
            .and(body_json(serde_json::json!({"unionid":"union-1"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"errcode":0,"result":{"userid":"member-1","contact_type":0}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path("/topapi/v2/user/get"))
            .and(query_param("access_token", "app-token")).and(body_json(serde_json::json!({"userid":"member-1"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"errcode":0,"result":{"userid":"member-1","unionid":"union-1","name":"赵高","title":"总监","dept_id_list":[2]}})))
            .expect(1).mount(&server).await;
        for (id, parent, name) in [(2, 1, "安全中心"), (1, 0, "公司")] {
            Mock::given(method("POST"))
                .and(path("/topapi/v2/department/get"))
                .and(query_param("access_token", "app-token"))
                .and(body_json(serde_json::json!({"dept_id":id})))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"errcode":0,"result":{"dept_id":id,"parent_id":parent,"name":name}}),
                ))
                .expect(1)
                .mount(&server)
                .await;
        }
        let mut profile = DingtalkProvider::to_provider_user_info(&info, "union-1");
        DingtalkProvider::enrich_profile_from(&server.uri(), &cfg, &info, &mut profile).await;
        assert_eq!(profile.external_id, "union-1");
        assert_eq!(profile.preferred_username, "赵高");
        assert_eq!(profile.job_title.as_deref(), Some("总监"));
        assert_eq!(profile.org_unit_path.as_deref(), Some("公司 / 安全中心"));
        server.verify().await;
        server.reset().await;
        Mock::given(path("/gettoken"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"errcode":60011,"access_token":"must-not-use"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut fallback = DingtalkProvider::to_provider_user_info(&info, "union-1");
        DingtalkProvider::enrich_profile_from(&server.uri(), &cfg, &info, &mut fallback).await;
        assert_eq!(fallback.external_id, "union-1");
        assert_eq!(fallback.preferred_username, "昵称");
        assert_eq!(fallback.job_title, None);
        assert_eq!(fallback.org_unit_path, None);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[test]
    fn enterprise_profile_overrides_nickname_without_changing_identity_or_exposing_mobile() {
        let info: DingtalkUserInfo =
            serde_json::from_value(serde_json::json!({"unionId":"union-1","nick":"昵称","mobile":"private"})).unwrap();
        let mut profile = DingtalkProvider::to_provider_user_info(&info, "union-1");
        apply_contact_profile(
            &mut profile,
            &serde_json::json!({"name":" 赵高 ","title":" 信息安全总监 ","mobile":"private"}),
        );
        assert_eq!(profile.external_id, "union-1");
        assert_eq!(profile.preferred_username, "赵高");
        assert_eq!(profile.job_title.as_deref(), Some("信息安全总监"));
        assert_eq!(profile.org_unit_path, None);
        apply_contact_profile(&mut profile, &serde_json::json!({"name":" ","title":" "}));
        assert_eq!(profile.preferred_username, "赵高");
        assert_eq!(profile.job_title, None);
        assert!(
            validate_contact_response(&serde_json::json!({"errcode":60011}))
                .unwrap_err()
                .to_string()
                .contains("60011")
        );
        assert!(validate_contact_response(&serde_json::json!({"result":{}})).is_err());
    }

    #[test]
    fn configured_corp_requires_matching_verified_token_organization() {
        let mut cfg = DingtalkProviderConfig {
            app_key: "app".into(),
            app_secret: "secret".into(),
            corp_id: Some("corp-1".into()),
            redirect_uri: Some("https://example.com/callback".into()),
            external_id_field: "unionId".into(),
        };
        assert!(
            DingtalkProvider::build_authorize_url(&cfg, "state").contains("scope=openid%20corpid%20Contact.User.Read")
        );
        for org in [None, Some("other-corp"), Some("corp-1")] {
            let data = DingtalkUserTokenResponse {
                access_token: Some("test-token".into()),
                corp_id: org.map(str::to_owned),
            };
            assert_eq!(
                DingtalkProvider::validate_token(&cfg, data).is_ok(),
                org == Some("corp-1")
            );
        }
        cfg.corp_id = None;
        assert!(DingtalkProvider::build_authorize_url(&cfg, "state").contains("scope=openid%20Contact.User.Read&"));
        assert!(
            DingtalkProvider::validate_token(
                &cfg,
                DingtalkUserTokenResponse {
                    access_token: Some("test-token".into()),
                    corp_id: None
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn build_authorize_url_contains_required_params() {
        let cfg = DingtalkProviderConfig {
            app_key: "ding_test".into(),
            app_secret: "secret".into(),
            corp_id: None,
            redirect_uri: Some("https://example.com/api/one/sso/dingtalk/callback".into()),
            external_id_field: "unionId".into(),
        };
        let url = DingtalkProvider::build_authorize_url(&cfg, "state456");
        assert!(url.contains("client_id=ding_test"));
        assert!(url.contains("state=state456"));
        assert!(url.contains("scope=openid%20Contact.User.Read&"));
    }

    #[test]
    fn resolve_external_id_prefers_configured_field() {
        let info = DingtalkUserInfo {
            union_id: Some("uid".into()),
            open_id: Some("oid".into()),
            nick: None,
            mobile: None,
        };
        assert_eq!(
            DingtalkProvider::resolve_external_id(&info, "unionId").as_deref(),
            Some("uid")
        );
        assert_eq!(
            DingtalkProvider::resolve_external_id(&info, "openId").as_deref(),
            Some("oid")
        );
    }
}
