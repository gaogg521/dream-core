//! WeCom (企业微信) OAuth provider.
//!
//! Translation of `WeComAuthProvider.ts`. Unlike Feishu/DingTalk, WeCom
//! uses a corp access token (not a user access token) + OAuth code →
//! corp UserId. The "external id" is the corp UserId.

use serde::{Deserialize, Serialize};

use crate::error::SsoError;
use crate::providers::ProviderUserInfo;

const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

const AUTHORIZE_URL: &str = "https://open.weixin.qq.com/connect/oauth2/authorize";
const GETTOKEN_URL: &str = "https://qyapi.weixin.qq.com/cgi-bin/gettoken";
const USER_INFO_URL: &str = "https://qyapi.weixin.qq.com/cgi-bin/user/getuserinfo";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WecomProviderConfig {
    pub corp_id: String,
    pub agent_id: String,
    pub secret: String,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default)]
    pub employee_no_field: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct WecomTokenResponse {
    errcode: Option<i64>,
    errmsg: Option<String>,
    access_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct WecomUserInfoResponse {
    errcode: Option<i64>,
    errmsg: Option<String>,
    #[serde(alias = "UserId", alias = "userid")]
    user_id: Option<String>,
    /// Parsed but unused: WeCom returns it for non-member visitors, and
    /// keeping it documents why `user_id` can be absent.
    #[allow(dead_code)]
    #[serde(alias = "OpenId", alias = "openid")]
    open_id: Option<String>,
}

pub struct WecomProvider;

impl WecomProvider {
    /// Standard-browser QR login, matching the official WwLogin SDK endpoint.
    pub fn build_web_authorize_url(config: &WecomProviderConfig, state: &str) -> String {
        format!(
            "https://open.work.weixin.qq.com/wwopen/sso/qrConnect?appid={}&agentid={}&redirect_uri={}&state={}",
            urlencode(&config.corp_id),
            urlencode(&config.agent_id),
            urlencode(config.redirect_uri.as_deref().unwrap_or("")),
            urlencode(state),
        )
    }

    pub fn build_authorize_url(config: &WecomProviderConfig, state: &str) -> String {
        let redirect = config.redirect_uri.as_deref().unwrap_or("");
        format!(
            "{AUTHORIZE_URL}?appid={}&redirect_uri={}&response_type=code&scope=snsapi_base&state={}&agentid={}#wechat_redirect",
            urlencode(&config.corp_id),
            urlencode(redirect),
            urlencode(state),
            urlencode(&config.agent_id),
        )
    }

    pub async fn fetch_corp_access_token(corp_id: &str, secret: &str) -> Result<String, SsoError> {
        let client = http_client()?;
        let resp = client
            .get(GETTOKEN_URL)
            .query(&[("corpid", corp_id), ("corpsecret", secret)])
            .send()
            .await?;
        let status = resp.status();
        let data: WecomTokenResponse = resp.json().await.unwrap_or_default();
        if !status.is_success() {
            return Err(SsoError::Internal(format!("WeCom API error: HTTP {status}")));
        }
        if data.errcode.unwrap_or(-1) != 0 {
            return Err(SsoError::Internal(
                data.errmsg.unwrap_or_else(|| "WeCom token request failed".into()),
            ));
        }
        data.access_token
            .filter(|s| !s.is_empty())
            .ok_or_else(|| SsoError::Internal("WeCom token request: missing access_token".into()))
    }

    /// Returns the corp UserId (the external id for WeCom).
    pub async fn fetch_user_id_by_code(access_token: &str, code: &str) -> Result<String, SsoError> {
        let client = http_client()?;
        let resp = client
            .get(USER_INFO_URL)
            .query(&[("access_token", access_token), ("code", code)])
            .send()
            .await?;
        let status = resp.status();
        let data: WecomUserInfoResponse = resp.json().await.unwrap_or_default();
        if !status.is_success() {
            return Err(SsoError::Internal(format!("WeCom user info failed: HTTP {status}")));
        }
        Self::validate_user_info(data)
    }

    fn validate_user_info(data: WecomUserInfoResponse) -> Result<String, SsoError> {
        if data.errcode.unwrap_or(-1) != 0 {
            return Err(SsoError::Internal(
                data.errmsg.unwrap_or_else(|| "WeCom user info failed".into()),
            ));
        }
        data.user_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| SsoError::Internal("WeCom user info: missing UserId".into()))
    }

    pub fn to_provider_user_info(user_id: &str) -> ProviderUserInfo {
        ProviderUserInfo {
            employee_no: None,
            enterprise_email: None,
            external_id: user_id.to_owned(),
            preferred_username: format!("wecom_{}", crate::providers::synthetic_username_suffix(user_id, 16)),
            org_unit_path: None,
            job_title: None,
            org_external_id: None,
        }
    }

    /// OAuth identifies the member; the application Contact API supplies their
    /// enterprise profile. Missing Contact permissions must not prevent login.
    pub async fn enrich_profile(access_token: &str, profile: &mut ProviderUserInfo, employee_no_field: Option<&str>) {
        Self::enrich_profile_from(
            "https://qyapi.weixin.qq.com/cgi-bin",
            access_token,
            profile,
            employee_no_field,
        )
        .await;
    }

    async fn enrich_profile_from(
        base: &str,
        access_token: &str,
        profile: &mut ProviderUserInfo,
        employee_no_field: Option<&str>,
    ) {
        let result = tokio::time::timeout(HTTP_TIMEOUT, async {
            let client = http_client()?;
            let data: serde_json::Value = client
                .get(format!("{base}/user/get"))
                .query(&[("access_token", access_token), ("userid", profile.external_id.as_str())])
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            validate_contact_response(&data)?;
            if data["userid"].as_str() != Some(profile.external_id.as_str()) {
                return Err(SsoError::IdentityMissing);
            }
            apply_contact_profile(profile, &data);
            profile.employee_no = employee_number(&data, employee_no_field.unwrap_or("工号"));
            if let Some(dept) = data["main_department"]
                .as_i64()
                .or_else(|| data["department"].as_array()?.first()?.as_i64())
            {
                let departments: serde_json::Value = client
                    .get(format!("{base}/department/list"))
                    .query(&[("access_token", access_token)])
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                validate_contact_response(&departments)?;
                if let Some(nodes) = departments["department"].as_array() {
                    profile.org_unit_path = department_path(nodes, dept);
                }
            }
            Ok::<(), SsoError>(())
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            tracing::warn!(
                provider = "wecom",
                "SSO contact profile incomplete; check application Contact permissions and visible member scope"
            );
        }
    }

    pub async fn test_credentials(corp_id: &str, secret: &str) -> Result<(), SsoError> {
        let cid = corp_id.trim();
        let sec = secret.trim();
        if cid.is_empty() || sec.is_empty() || sec == "******" {
            return Err(SsoError::BadRequest(
                "CorpId and Secret are required for connection test".into(),
            ));
        }
        Self::fetch_corp_access_token(cid, sec).await.map(|_| ())
    }
}

fn validate_contact_response(data: &serde_json::Value) -> Result<(), SsoError> {
    if data["errcode"].as_i64() != Some(0) {
        return Err(SsoError::Internal(format!(
            "WeCom Contact API failed (code {:?})",
            data["errcode"].as_i64()
        )));
    }
    Ok(())
}

fn apply_contact_profile(profile: &mut ProviderUserInfo, data: &serde_json::Value) {
    profile.enterprise_email = data["biz_mail"]
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    // WeCom has no standard employee-number field. Only an explicitly named custom attribute is eligible.
    profile.employee_no = employee_number(data, "工号");
    if let Some(name) = data["name"].as_str().map(str::trim).filter(|name| !name.is_empty()) {
        profile.preferred_username = name.to_owned();
    }
    profile.job_title = data["position"]
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned);
}

fn employee_number(data: &serde_json::Value, field: &str) -> Option<String> {
    data.pointer("/extattr/attrs")?
        .as_array()?
        .iter()
        .find(|v| v["name"].as_str() == Some(field.trim()) && v["type"].as_i64() == Some(0))?
        .pointer("/text/value")?
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn department_path(nodes: &[serde_json::Value], mut id: i64) -> Option<String> {
    let mut names = Vec::new();
    let mut visited = std::collections::HashSet::new();
    while id > 0 && visited.insert(id) {
        let Some(node) = nodes.iter().find(|node| node["id"].as_i64() == Some(id)) else {
            break;
        };
        if let Some(name) = node["name"].as_str().filter(|name| !name.trim().is_empty()) {
            names.push(name.trim().to_owned());
        }
        id = node["parentid"].as_i64().unwrap_or(0);
    }
    names.reverse();
    (!names.is_empty()).then(|| names.join(" / "))
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
            matchers::{method, path, query_param},
        };
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/get"))
            .and(query_param("access_token", "app-token"))
            .and(query_param("userid", "member-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"errcode":0,"userid":"member-1","name":"赵高","position":"总监","department":[2]}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET")).and(path("/department/list"))
            .and(query_param("access_token", "app-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"errcode":0,"department":[{"id":1,"parentid":0,"name":"公司"},{"id":2,"parentid":1,"name":"安全中心"}]})))
            .expect(1).mount(&server).await;
        let mut profile = WecomProvider::to_provider_user_info("member-1");
        WecomProvider::enrich_profile_from(&server.uri(), "app-token", &mut profile, None).await;
        assert_eq!(profile.external_id, "member-1");
        assert_eq!(profile.preferred_username, "赵高");
        assert_eq!(profile.job_title.as_deref(), Some("总监"));
        assert_eq!(profile.org_unit_path.as_deref(), Some("公司 / 安全中心"));
        server.verify().await;
        server.reset().await;
        Mock::given(path("/user/get"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"errcode":48002,"name":"must not apply"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut fallback = WecomProvider::to_provider_user_info("member-1");
        WecomProvider::enrich_profile_from(&server.uri(), "app-token", &mut fallback, None).await;
        assert_eq!(fallback.preferred_username, "wecom_member-1");
        assert_eq!(fallback.job_title, None);
        assert_eq!(fallback.org_unit_path, None);
    }

    #[test]
    fn employee_number_uses_only_the_named_text_attribute() {
        let data = serde_json::json!({"userid":"not-an-employee-number","extattr":{"attrs":[{"name":"Staff ID","type":0,"text":{"value":" E-1027 "}},{"name":"工号","type":1,"web":{"url":"https://example.test"}}]}});
        assert_eq!(employee_number(&data, "Staff ID").as_deref(), Some("E-1027"));
        assert!(employee_number(&data, "工号").is_none());
        assert!(employee_number(&data, "Missing").is_none());
    }

    #[test]
    fn contact_profile_keeps_identity_and_maps_staff_fields_and_department_hierarchy() {
        let mut profile = WecomProvider::to_provider_user_info("member-1");
        apply_contact_profile(
            &mut profile,
            &serde_json::json!({"name":" 赵高 ","position":" 信息安全总监 ","mobile":"private"}),
        );
        assert_eq!(profile.external_id, "member-1");
        assert_eq!(profile.preferred_username, "赵高");
        assert_eq!(profile.job_title.as_deref(), Some("信息安全总监"));
        assert_eq!(profile.org_unit_path, None);
        let nodes = vec![
            serde_json::json!({"id":1,"parentid":0,"name":"公司"}),
            serde_json::json!({"id":2,"parentid":1,"name":"安全中心"}),
        ];
        assert_eq!(department_path(&nodes, 2).as_deref(), Some("公司 / 安全中心"));
        assert_eq!(department_path(&nodes, 99), None);
        let cycle = vec![serde_json::json!({"id":2,"parentid":2,"name":"安全中心"})];
        assert_eq!(department_path(&cycle, 2).as_deref(), Some("安全中心"));
        assert!(
            validate_contact_response(&serde_json::json!({"errcode":48002}))
                .unwrap_err()
                .to_string()
                .contains("48002")
        );
        assert!(validate_contact_response(&serde_json::json!({"name":"invalid"})).is_err());
    }

    #[test]
    fn official_user_info_response_accepts_members_and_rejects_errors_or_visitors() {
        for key in ["UserId", "userid"] {
            let data = serde_json::from_value(serde_json::json!({"errcode":0,"errmsg":"ok",key:" member-1 "})).unwrap();
            assert_eq!(WecomProvider::validate_user_info(data).unwrap(), "member-1");
        }
        for response in [
            serde_json::json!({"errcode":40029,"errmsg":"invalid code","UserId":"member-1"}),
            serde_json::json!({"errcode":0,"OpenId":"visitor-1"}),
            serde_json::json!({"UserId":"member-1"}),
            serde_json::json!({"errcode":0,"UserId":" "}),
        ] {
            assert!(WecomProvider::validate_user_info(serde_json::from_value(response).unwrap()).is_err());
        }
    }

    #[test]
    fn browser_qr_login_uses_web_endpoint_and_encodes_callback_and_state() {
        let cfg = WecomProviderConfig {
            employee_no_field: None,
            corp_id: "ww_test".into(),
            agent_id: "1000002".into(),
            secret: "not-in-url".into(),
            redirect_uri: Some("https://example.com:8443/api/one/sso/wecom/callback".into()),
        };
        let url = WecomProvider::build_web_authorize_url(&cfg, "state&next=other");
        assert!(url.starts_with("https://open.work.weixin.qq.com/wwopen/sso/qrConnect?"));
        assert!(url.contains("appid=ww_test&agentid=1000002"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Fexample.com%3A8443%2Fapi%2Fone%2Fsso%2Fwecom%2Fcallback"));
        assert!(url.contains("state=state%26next%3Dother"));
        assert!(!url.contains("not-in-url"));
        assert!(!url.contains("snsapi_base"));
        assert!(WecomProvider::build_authorize_url(&cfg, "s").contains("scope=snsapi_base"));
    }

    #[test]
    fn build_authorize_url_contains_required_params() {
        let cfg = WecomProviderConfig {
            employee_no_field: None,
            corp_id: "corp_abc".into(),
            agent_id: "1000002".into(),
            secret: "secret".into(),
            redirect_uri: Some("https://example.com/api/one/sso/wecom/callback".into()),
        };
        let url = WecomProvider::build_authorize_url(&cfg, "state789");
        assert!(url.contains("appid=corp_abc"));
        assert!(url.contains("agentid=1000002"));
        assert!(url.contains("state=state789"));
        assert!(url.ends_with("#wechat_redirect"));
    }
}
