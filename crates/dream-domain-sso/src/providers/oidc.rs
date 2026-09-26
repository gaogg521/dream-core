//! Standard OpenID Connect (OIDC) provider.
//!
//! One provider covers the three big Western enterprise IdPs — Okta, Azure AD
//! (Microsoft Entra), and Google Workspace — because they all speak OIDC. The
//! flow is the textbook authorization-code flow with OIDC Discovery:
//!
//! 1. `discover` → GET `{issuer}/.well-known/openid-configuration` to learn the
//!    authorization / token / userinfo endpoints (all three IdPs publish it).
//! 2. `build_authorize_url` → redirect the browser to the IdP.
//! 3. `exchange_code` → POST the code to the token endpoint, get an access
//!    token (+ id_token).
//! 4. Verify `id_token` against the discovery `jwks_uri` (see `oidc_jwks`).
//!    Subject must match userinfo; mismatch is an error, not a choice.
//! 5. `fetch_user_info` → GET the userinfo endpoint with the access token to
//!    read the identity claims used for JIT provisioning.
//!
//! # Security note
//! The access token is still obtained from the token endpoint over TLS with
//! the client secret. JWKS verification of `id_token` is **defense in depth**,
//! not a patch for a broken chain. JWKS fetch failure is fail-closed — see
//! `oidc_jwks` module docs.

use serde::Deserialize;

use crate::error::SsoError;
use crate::providers::ProviderUserInfo;

const OIDC_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";
const DEFAULT_SCOPES: &str = "openid profile email";
const DEFAULT_EXTERNAL_ID_CLAIM: &str = "sub";
const DEFAULT_NAME_CLAIM: &str = "name";

/// Stored config for an OIDC provider. Keys are camelCase in the persisted
/// JSON (matching the other providers and the admin form); parsed manually in
/// `service::parse_oidc_config`, same as `parse_feishu_config`.
#[derive(Debug, Clone)]
pub struct OidcProviderConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    /// Space-separated scopes; must include `openid`.
    pub scopes: String,
    /// Claim holding the stable per-user id (default `sub`).
    pub external_id_claim: String,
    /// Claim holding the display name (default `name`).
    pub name_claim: String,
    /// Optional claim holding a company/tenant id (e.g. Google Workspace `hd`).
    /// When present it becomes `ProviderUserInfo.org_external_id`, so same-
    /// company logins auto-join the SSO enterprise tenant — mirrors the Feishu
    /// `tenant_key` semantics. `None` disables company binding for this IdP.
    pub company_claim: Option<String>,
    /// Test-only override for the discovery host (points at a wiremock server);
    /// never set in production, never surfaced in the admin form. Same pattern
    /// as `FeishuProviderConfig::base_url`.
    pub base_url: Option<String>,
}

impl OidcProviderConfig {
    /// Host the discovery document is fetched from. Production uses the
    /// `issuer`; tests override it. The token / userinfo / authorize endpoints
    /// themselves come from the discovery document (absolute URLs), so they
    /// follow the mock server automatically in tests.
    fn discovery_base(&self) -> &str {
        self.base_url.as_deref().unwrap_or(&self.issuer)
    }

    pub fn scopes_or_default(&self) -> &str {
        if self.scopes.trim().is_empty() {
            DEFAULT_SCOPES
        } else {
            self.scopes.as_str()
        }
    }

    pub fn external_id_claim_or_default(&self) -> &str {
        if self.external_id_claim.trim().is_empty() {
            DEFAULT_EXTERNAL_ID_CLAIM
        } else {
            self.external_id_claim.as_str()
        }
    }

    pub fn name_claim_or_default(&self) -> &str {
        if self.name_claim.trim().is_empty() {
            DEFAULT_NAME_CLAIM
        } else {
            self.name_claim.as_str()
        }
    }
}

/// The subset of the OIDC discovery document we use.
#[derive(Debug, Clone, Deserialize)]
pub struct OidcDiscovery {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    /// Optional per spec, but present on Okta/Azure/Google. Required for our
    /// userinfo-based identity read.
    pub userinfo_endpoint: Option<String>,
    /// JWKS URL used to verify `id_token`. Required for login (fail-closed).
    pub jwks_uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct OidcTokenResponse {
    access_token: Option<String>,
    id_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OidcTokens {
    pub access_token: String,
    pub id_token: String,
}

pub struct OidcProvider;

impl OidcProvider {
    pub(crate) fn client() -> Result<reqwest::Client, SsoError> {
        reqwest::Client::builder()
            .timeout(OIDC_HTTP_TIMEOUT)
            .build()
            .map_err(|e| SsoError::Internal(format!("http client: {e}")))
    }

    /// Fetch + parse the OIDC discovery document.
    pub async fn discover(config: &OidcProviderConfig) -> Result<OidcDiscovery, SsoError> {
        let base = config.discovery_base().trim_end_matches('/');
        let url = format!("{base}{DISCOVERY_PATH}");
        let resp = Self::client()?.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(SsoError::Internal(format!("OIDC discovery failed: HTTP {status}")));
        }
        let discovery: OidcDiscovery = resp
            .json()
            .await
            .map_err(|e| SsoError::Internal(format!("OIDC discovery parse: {e}")))?;
        if discovery.authorization_endpoint.trim().is_empty() || discovery.token_endpoint.trim().is_empty() {
            return Err(SsoError::Internal(
                "OIDC discovery missing authorization_endpoint/token_endpoint".into(),
            ));
        }
        Ok(discovery)
    }

    pub fn build_authorize_url(
        discovery: &OidcDiscovery,
        config: &OidcProviderConfig,
        state: &str,
        nonce: &str,
    ) -> String {
        format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&nonce={}",
            discovery.authorization_endpoint,
            urlencode(&config.client_id),
            urlencode(&config.redirect_uri),
            urlencode(config.scopes_or_default()),
            urlencode(state),
            urlencode(nonce),
        )
    }

    /// Exchange the authorization code for access + id tokens.
    pub async fn exchange_code(
        discovery: &OidcDiscovery,
        config: &OidcProviderConfig,
        code: &str,
    ) -> Result<OidcTokens, SsoError> {
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", &config.client_id),
            ("client_secret", &config.client_secret),
        ];
        if !config.redirect_uri.is_empty() {
            form.push(("redirect_uri", &config.redirect_uri));
        }

        let resp = Self::client()?
            .post(&discovery.token_endpoint)
            .form(&form)
            .send()
            .await?;
        let status = resp.status();
        let json: serde_json::Value = resp.json().await.unwrap_or_default();
        if !status.is_success() {
            let err = json
                .get("error_description")
                .or_else(|| json.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or("token exchange failed");
            return Err(SsoError::Internal(format!("OIDC token exchange: HTTP {status}: {err}")));
        }
        let token: OidcTokenResponse = serde_json::from_value(json).unwrap_or_default();
        let access_token = token
            .access_token
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| SsoError::Internal("OIDC token exchange: missing access_token".into()))?;
        let id_token = token
            .id_token
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| SsoError::Internal("OIDC token exchange: missing id_token".into()))?;
        Ok(OidcTokens { access_token, id_token })
    }

    /// Code exchange + JWKS verify + userinfo, with subject agreement.
    pub async fn complete_login(
        discovery: &OidcDiscovery,
        config: &OidcProviderConfig,
        code: &str,
        nonce: &str,
        cache: &crate::providers::oidc_jwks::JwksCache,
    ) -> Result<ProviderUserInfo, SsoError> {
        let tokens = Self::exchange_code(discovery, config, code).await?;
        let client = Self::client()?;
        let id_claims =
            crate::providers::oidc_jwks::verify_id_token(&client, cache, discovery, config, &tokens.id_token, nonce)
                .await?;
        let userinfo = Self::fetch_user_info(discovery, &tokens.access_token).await?;
        crate::providers::oidc_jwks::identities_must_match(
            &id_claims,
            &userinfo,
            config.external_id_claim_or_default(),
        )?;
        let external_id = Self::resolve_external_id(&userinfo, config.external_id_claim_or_default())
            .ok_or(SsoError::IdentityMissing)?;
        Ok(Self::to_provider_user_info(&userinfo, &external_id, config))
    }

    /// Read identity claims from the userinfo endpoint using the access token.
    pub async fn fetch_user_info(discovery: &OidcDiscovery, access_token: &str) -> Result<serde_json::Value, SsoError> {
        let endpoint = discovery
            .userinfo_endpoint
            .as_deref()
            .filter(|e| !e.trim().is_empty())
            .ok_or_else(|| SsoError::Internal("OIDC provider has no userinfo_endpoint".into()))?;
        let resp = Self::client()?.get(endpoint).bearer_auth(access_token).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(SsoError::Internal(format!("OIDC userinfo failed: HTTP {status}")));
        }
        let claims: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| SsoError::Internal(format!("OIDC userinfo parse: {e}")))?;
        Ok(claims)
    }

    /// Resolve the external id from the configured claim (default `sub`).
    pub fn resolve_external_id(claims: &serde_json::Value, claim: &str) -> Option<String> {
        claims
            .get(claim)
            .and_then(claim_as_string)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    /// Map OIDC claims onto the normalized `ProviderUserInfo`.
    pub fn to_provider_user_info(
        claims: &serde_json::Value,
        external_id: &str,
        config: &OidcProviderConfig,
    ) -> ProviderUserInfo {
        let preferred = claims
            .get(config.name_claim_or_default())
            .and_then(claim_as_string)
            .or_else(|| claims.get("preferred_username").and_then(claim_as_string))
            .or_else(|| claims.get("name").and_then(claim_as_string))
            .or_else(|| claims.get("email").and_then(claim_as_string))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("oidc_{}", crate::providers::synthetic_username_suffix(external_id, 16)));

        // Company/tenant id from the optional configured claim only — OIDC has
        // no standard company claim, so this stays None unless the admin maps
        // one (e.g. Google `hd`). Empty/blank never binds an enterprise to "".
        let org_external_id = config
            .company_claim
            .as_deref()
            .filter(|c| !c.trim().is_empty())
            .and_then(|c| claims.get(c))
            .and_then(claim_as_string)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());

        ProviderUserInfo {
            external_id: external_id.to_owned(),
            preferred_username: preferred,
            // Standard OIDC claims carry neither department nor job title —
            // those arrive via SCIM / directory sync in a later step.
            org_unit_path: None,
            job_title: None,
            org_external_id,
        }
    }

    /// Validate the config by resolving the discovery document — used by the
    /// admin "Test connection" button.
    pub async fn test_credentials(config: &OidcProviderConfig) -> Result<(), SsoError> {
        if config.issuer.trim().is_empty() || config.client_id.trim().is_empty() {
            return Err(SsoError::BadRequest(
                "Issuer and Client ID are required for connection test".into(),
            ));
        }
        let discovery = Self::discover(config).await?;
        if discovery
            .userinfo_endpoint
            .as_deref()
            .map(|e| e.trim().is_empty())
            .unwrap_or(true)
        {
            return Err(SsoError::Internal(
                "OIDC issuer's discovery document has no userinfo_endpoint".into(),
            ));
        }
        if discovery
            .jwks_uri
            .as_deref()
            .map(|e| e.trim().is_empty())
            .unwrap_or(true)
        {
            return Err(SsoError::Internal(
                "OIDC issuer's discovery document has no jwks_uri".into(),
            ));
        }
        Ok(())
    }
}

/// Accept a claim that is a JSON string, or coerce a number/bool to its string
/// form (some IdPs emit numeric `sub`).
pub(crate) fn claim_as_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config_with_base(base: &str) -> OidcProviderConfig {
        OidcProviderConfig {
            issuer: "https://issuer.example.com".into(),
            client_id: "client-abc".into(),
            client_secret: "secret-xyz".into(),
            redirect_uri: "https://app.example.com/api/one/sso/oidc/callback".into(),
            scopes: String::new(),
            external_id_claim: String::new(),
            name_claim: String::new(),
            company_claim: Some("hd".into()),
            base_url: Some(base.to_owned()),
        }
    }

    async fn mount_discovery(server: &MockServer) {
        let base = server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": "https://issuer.example.com",
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "userinfo_endpoint": format!("{base}/userinfo"),
                "jwks_uri": format!("{base}/jwks"),
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn discover_parses_endpoints() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        assert_eq!(d.authorization_endpoint, format!("{}/authorize", server.uri()));
        assert_eq!(d.token_endpoint, format!("{}/token", server.uri()));
        assert_eq!(
            d.userinfo_endpoint.as_deref(),
            Some(format!("{}/userinfo", server.uri()).as_str())
        );
    }

    #[tokio::test]
    async fn build_authorize_url_contains_required_params() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let url = OidcProvider::build_authorize_url(&d, &cfg, "state123", "nonce-xyz");
        assert!(url.starts_with(&format!("{}/authorize?", server.uri())));
        assert!(url.contains("client_id=client-abc"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("state=state123"));
        assert!(url.contains("nonce=nonce-xyz"));
        // default scopes url-encoded ("openid profile email")
        assert!(url.contains("scope=openid%20profile%20email"));
    }

    #[tokio::test]
    async fn exchange_code_returns_access_token() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-123", "id_token": "jwt.here", "token_type": "Bearer"
            })))
            .mount(&server)
            .await;
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let token = OidcProvider::exchange_code(&d, &cfg, "code-abc").await.unwrap();
        assert_eq!(token.access_token, "at-123");
        assert_eq!(token.id_token, "jwt.here");
    }

    #[tokio::test]
    async fn exchange_code_surfaces_provider_error() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant", "error_description": "code expired"
            })))
            .mount(&server)
            .await;
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let err = OidcProvider::exchange_code(&d, &cfg, "bad").await.unwrap_err();
        assert!(format!("{err}").contains("code expired"));
    }

    #[tokio::test]
    async fn fetch_user_info_maps_claims() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sub": "okta-user-1", "name": "Jane Doe", "email": "jane@acme.com", "hd": "acme.com"
            })))
            .mount(&server)
            .await;
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let claims = OidcProvider::fetch_user_info(&d, "at-123").await.unwrap();

        let external_id = OidcProvider::resolve_external_id(&claims, cfg.external_id_claim_or_default()).unwrap();
        assert_eq!(external_id, "okta-user-1");
        let profile = OidcProvider::to_provider_user_info(&claims, &external_id, &cfg);
        assert_eq!(profile.preferred_username, "Jane Doe");
        assert_eq!(profile.external_id, "okta-user-1");
        // company_claim "hd" → org_external_id (auto-join key).
        assert_eq!(profile.org_external_id.as_deref(), Some("acme.com"));
        assert_eq!(profile.org_unit_path, None);
        assert_eq!(profile.job_title, None);
    }

    #[test]
    fn to_provider_user_info_falls_back_to_email_then_prefix() {
        let cfg = OidcProviderConfig {
            issuer: "https://i".into(),
            client_id: "c".into(),
            client_secret: "s".into(),
            redirect_uri: String::new(),
            scopes: String::new(),
            external_id_claim: String::new(),
            name_claim: String::new(),
            company_claim: None,
            base_url: None,
        };
        // No name → email.
        let claims = serde_json::json!({ "sub": "u1", "email": "bob@x.com" });
        let p = OidcProvider::to_provider_user_info(&claims, "u1", &cfg);
        assert_eq!(p.preferred_username, "bob@x.com");
        // No name/email → oidc_ prefix.
        let claims = serde_json::json!({ "sub": "user_1234567890" });
        let p = OidcProvider::to_provider_user_info(&claims, "user_1234567890", &cfg);
        assert!(p.preferred_username.starts_with("oidc_"));
        // company_claim None → org_external_id None even if `hd` present.
        let claims = serde_json::json!({ "sub": "u1", "hd": "acme.com" });
        let p = OidcProvider::to_provider_user_info(&claims, "u1", &cfg);
        assert_eq!(p.org_external_id, None);
    }

    #[tokio::test]
    async fn test_credentials_ok_and_rejects_bad_issuer() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        let cfg = config_with_base(&server.uri());
        OidcProvider::test_credentials(&cfg).await.unwrap();

        // Unreachable discovery host → error.
        let mut bad = config_with_base("http://127.0.0.1:1");
        bad.base_url = Some("http://127.0.0.1:1".into());
        assert!(OidcProvider::test_credentials(&bad).await.is_err());

        // Missing issuer/client_id → BadRequest before any network call.
        let mut blank = config_with_base(&server.uri());
        blank.issuer = String::new();
        assert!(OidcProvider::test_credentials(&blank).await.is_err());
    }

    const TEST_RSA_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEuwIBADANBgkqhkiG9w0BAQEFAASCBKUwggShAgEAAoIBAQCpK22P7sz5v9/t
6BG2+kb8n6Xx3hINopbcy19mjlkdaCAt5bqBi/9ZSQoYBs2jZftg0DJw067sV8qY
15ISLn7MIWT+CxCvJ3oMYd62zRaDJVVSCwh0ivMM3z0ulUd5xpqJDTAqxuu7q2yG
v91Q3L9kSTDPagu3SEq6f8mT/g39WJ6na/7sNT6Xu9uNRgyUtLzS70LT4GtsUeFH
EUB2dplD15Qb8oM4v1+WQJHNyBU7hp3CD48qI71zcHw1pOeshGJHC/Mennmhzkvp
OpVjtky2RTbUSHKT+zE5c5TBoCJIyqP6ErX7TJcIuGW0WdPF0hJv70/ISUEGsKl1
6FjdNGexAgMBAAECgf9IEchLWWDZxXSQ1h015sn3Ncxsjj8CsBG1Xq718g7lCEct
RoF+TzYpw4QZWEyjH/9H72qNxqDu7zfQhYYlWMmMDW4JDI2/EQJd5BQNrLG6jV0b
5rdjbw68nR5jihU5O/L6EDRFBRnIie9iOLsAiArBnqy8sGDtZE7xxR8LfWBYXYdo
lVcVT/dUQehV/XTjhxVJ9dDzIjlURp6cfiyPtbw3iip5NOEo07kJFMKqIZKTpFPb
5ofY3a6jlucVFIhqeUPsbwozRP5CbNCJgRq4gaXywxZ62Jif3/zEkAbEeUluJzwJ
pNvwakIZl0OpTaF2qmrT38iFkiKKLXqOa1XwMeECgYEA0OQyZGJbVCvyTvO7iIL1
+30G9kgC+vKHrxRcmPpvo+kNGkbkGFSXuGXoR3hkvT7XZbrOQXNp661GafehGhI9
elOeH65WJOK/FJAx9upYu52QnEFT+Mr75QxTfOf2paRZA+QiVDgJ5gerZIaRFkAo
q86L3yd8d6VhRIKzieoyPVECgYEAz1H/qIZEA52VD2NalyHF3OgSAoZoZrvOt6vE
RMWBWP4D0a1zshBqPXGmFTyOW8DpieM3ljEG6tlU9TtEhd+Bmkqw9Lgs4hoPxp7Z
on5jAk/uLJB4L1P+mEY9xTbRtAdRHDChjNh51FMeomb8uMnFQtmgNXT784SqdGRC
/7I9bGECgYEAj8XwVR1JRMa2kNa6pXuVuFFWYF4yBuy0rLE0BmqgOk2mIgbW6VQX
1Of3FnHrzEEbWb5YRb4dEgQB6d9xN5OEUtSIib+hNOQHpiyU5yBmkEMjjBh+pkd3
Vi/EqryxC1LxnXcAlby4O2Xd9mOUKp9gHtgbdy0jQupF5zSaQ/s4NvECgYBBqoFF
ybFFS+Zox1lsQUBApij+L8BludrSBk/WUJCVtW9UPJJGtjhQWez3EQUuPr459IQo
yEKepFPqkOk1VgPg8QN3n9Znj0Wr7aiVdV663sJbzy6iHwKnDKiIDMMDOMYSHb0t
tWtxOxqa6e/mP9KBSBkclX8wNLcgwpkOEFCwQQKBgGymrIXtSmUten15VURPMbq8
D30lOkZWOC5r3a5+HaE3nQkLzy4OkqqY6YzaaSxZ8Gi/T+S3D2IemA+3wpMs2OiF
1mbVrUxD+iF2zGsS27wOhaCUcOBB6tmogfmZxLiXUwDtwXTrM5aqZAgbY+/tHVmn
LHIbJB+Lsdt0yBmT2Gm0
-----END PRIVATE KEY-----
";

    const TEST_KID: &str = "test-kid-1";
    const TEST_NONCE: &str = "nonce-login-1";
    const TEST_SUB: &str = "okta-user-1";

    fn test_encoding_key() -> jsonwebtoken::EncodingKey {
        jsonwebtoken::EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).expect("test rsa pem")
    }

    fn test_jwks_json(kid: &str) -> serde_json::Value {
        let mut jwk = jsonwebtoken::jwk::Jwk::from_encoding_key(&test_encoding_key(), jsonwebtoken::Algorithm::RS256)
            .expect("jwk from encoding key");
        jwk.common.key_id = Some(kid.to_owned());
        serde_json::json!({ "keys": [jwk] })
    }

    fn sign_id_token(kid: &str, nonce: &str, sub: &str, exp_offset_secs: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims = serde_json::json!({
            "sub": sub,
            "iss": "https://issuer.example.com",
            "aud": "client-abc",
            "exp": now + exp_offset_secs,
            "iat": now,
            "nonce": nonce,
        });
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(kid.to_owned());
        jsonwebtoken::encode(&header, &claims, &test_encoding_key()).expect("sign")
    }

    async fn mount_jwks(server: &MockServer, kid: &str) {
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks_json(kid)))
            .mount(server)
            .await;
    }

    async fn mount_userinfo(server: &MockServer, sub: &str) {
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sub": sub, "name": "Jane Doe", "email": "jane@acme.com", "hd": "acme.com"
            })))
            .mount(server)
            .await;
    }

    async fn mount_token(server: &MockServer, id_token: &str) {
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-123", "id_token": id_token, "token_type": "Bearer"
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn complete_login_verifies_id_token_and_matches_userinfo() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        mount_jwks(&server, TEST_KID).await;
        let id_token = sign_id_token(TEST_KID, TEST_NONCE, TEST_SUB, 3600);
        mount_token(&server, &id_token).await;
        mount_userinfo(&server, TEST_SUB).await;
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        let profile = OidcProvider::complete_login(&d, &cfg, "code", TEST_NONCE, &cache)
            .await
            .unwrap();
        assert_eq!(profile.external_id, TEST_SUB);
        assert_eq!(profile.preferred_username, "Jane Doe");
    }

    #[tokio::test]
    async fn complete_login_rejects_when_userinfo_sub_disagrees() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        mount_jwks(&server, TEST_KID).await;
        let id_token = sign_id_token(TEST_KID, TEST_NONCE, TEST_SUB, 3600);
        mount_token(&server, &id_token).await;
        mount_userinfo(&server, "someone-else").await;
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        let err = OidcProvider::complete_login(&d, &cfg, "code", TEST_NONCE, &cache)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("does not match userinfo"));
    }

    #[tokio::test]
    async fn id_token_tampered_signature_is_rejected() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        mount_jwks(&server, TEST_KID).await;
        let mut id_token = sign_id_token(TEST_KID, TEST_NONCE, TEST_SUB, 3600);
        let last = id_token.pop().unwrap();
        id_token.push(if last == 'A' { 'B' } else { 'A' });
        mount_token(&server, &id_token).await;
        mount_userinfo(&server, TEST_SUB).await;
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        assert!(
            OidcProvider::complete_login(&d, &cfg, "code", TEST_NONCE, &cache)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn unknown_kid_refreshes_jwks_then_rejects_if_still_missing() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks_json("other-kid")))
            .expect(2)
            .mount(&server)
            .await;
        let id_token = sign_id_token(TEST_KID, TEST_NONCE, TEST_SUB, 3600);
        mount_token(&server, &id_token).await;
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        let client = OidcProvider::client().unwrap();
        let jwks_uri = d.jwks_uri.as_deref().unwrap();
        cache.key_for_kid(&client, jwks_uri, Some("other-kid")).await.unwrap();
        let err = crate::providers::oidc_jwks::verify_id_token(&client, &cache, &d, &cfg, &id_token, TEST_NONCE)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("not present in JWKS after refresh"));
        server.verify().await;
    }

    #[tokio::test]
    async fn unknown_kid_succeeds_after_jwks_rotation_refresh() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks_json("stale-kid")))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks_json(TEST_KID)))
            .mount(&server)
            .await;
        let id_token = sign_id_token(TEST_KID, TEST_NONCE, TEST_SUB, 3600);
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        let client = OidcProvider::client().unwrap();
        cache
            .key_for_kid(&client, d.jwks_uri.as_deref().unwrap(), Some("stale-kid"))
            .await
            .unwrap();
        crate::providers::oidc_jwks::verify_id_token(&client, &cache, &d, &cfg, &id_token, TEST_NONCE)
            .await
            .unwrap();
    }

    #[test]
    fn alg_none_header_is_detected() {
        assert!(crate::providers::oidc_jwks::test_header_alg_is_none(
            "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJ1In0."
        ));
        assert!(!crate::providers::oidc_jwks::test_header_alg_is_none(&sign_id_token(
            TEST_KID, TEST_NONCE, TEST_SUB, 3600
        )));
    }

    #[tokio::test]
    async fn alg_none_id_token_is_rejected_without_hitting_jwks() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks_json(TEST_KID)))
            .expect(0)
            .mount(&server)
            .await;
        let none_token = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJ1In0.";
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        let client = OidcProvider::client().unwrap();
        let err = crate::providers::oidc_jwks::verify_id_token(&client, &cache, &d, &cfg, none_token, TEST_NONCE)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("alg none"));
        server.verify().await;
    }

    #[tokio::test]
    async fn expired_id_token_is_rejected() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        mount_jwks(&server, TEST_KID).await;
        let id_token = sign_id_token(TEST_KID, TEST_NONCE, TEST_SUB, -120);
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        let client = OidcProvider::client().unwrap();
        let err = crate::providers::oidc_jwks::verify_id_token(&client, &cache, &d, &cfg, &id_token, TEST_NONCE)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("id_token verify"));
    }

    #[tokio::test]
    async fn nonce_mismatch_is_rejected() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        mount_jwks(&server, TEST_KID).await;
        let id_token = sign_id_token(TEST_KID, "wrong-nonce", TEST_SUB, 3600);
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        let client = OidcProvider::client().unwrap();
        let err = crate::providers::oidc_jwks::verify_id_token(&client, &cache, &d, &cfg, &id_token, TEST_NONCE)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("nonce mismatch"));
    }

    #[tokio::test]
    async fn wrong_issuer_or_audience_is_rejected() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        mount_jwks(&server, TEST_KID).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(TEST_KID.to_owned());
        let bad_iss = jsonwebtoken::encode(
            &header,
            &serde_json::json!({
                "sub": TEST_SUB,
                "iss": "https://evil.example.com",
                "aud": "client-abc",
                "exp": now + 3600,
                "iat": now,
                "nonce": TEST_NONCE,
            }),
            &test_encoding_key(),
        )
        .unwrap();
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        let client = OidcProvider::client().unwrap();
        assert!(
            crate::providers::oidc_jwks::verify_id_token(&client, &cache, &d, &cfg, &bad_iss, TEST_NONCE)
                .await
                .is_err()
        );
        let bad_aud = jsonwebtoken::encode(
            &header,
            &serde_json::json!({
                "sub": TEST_SUB,
                "iss": "https://issuer.example.com",
                "aud": "someone-else",
                "exp": now + 3600,
                "iat": now,
                "nonce": TEST_NONCE,
            }),
            &test_encoding_key(),
        )
        .unwrap();
        assert!(
            crate::providers::oidc_jwks::verify_id_token(&client, &cache, &d, &cfg, &bad_aud, TEST_NONCE)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn jwks_endpoint_down_fails_closed() {
        let server = MockServer::start().await;
        mount_discovery(&server).await;
        let id_token = sign_id_token(TEST_KID, TEST_NONCE, TEST_SUB, 3600);
        let cfg = config_with_base(&server.uri());
        let d = OidcProvider::discover(&cfg).await.unwrap();
        let cache = crate::providers::oidc_jwks::JwksCache::new();
        let client = OidcProvider::client().unwrap();
        let err = crate::providers::oidc_jwks::verify_id_token(&client, &cache, &d, &cfg, &id_token, TEST_NONCE)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("JWKS"));
    }
}
