//! Explicit-login OAuth discovery. Status checks never call this module.
//! References: RFC 9728 sections 3/5, RFC 8414 section 3, RFC 7591.
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

use crate::error::McpError;

const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Metadata {
    pub issuer: Option<String>,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub code_challenge_methods_supported: Option<Vec<String>>,
    #[serde(skip)]
    pub resource: String,
}

#[derive(Debug, Deserialize)]
struct ResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
}

/// Public registration values, persisted atomically with their issued token.
/// Confidential-client secrets are deliberately not part of this format.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ClientInfo {
    pub client_id: String,
    pub issuer: Option<String>,
    pub resource: String,
}

pub(crate) fn validate_url(value: &str) -> Result<Url, McpError> {
    let url = Url::parse(value).map_err(|_| McpError::OAuth("Invalid OAuth URL".into()))?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(McpError::OAuth(
            "OAuth endpoints require HTTPS (HTTP is allowed only on loopback), without credentials or fragments".into(),
        ));
    }
    Ok(url)
}

pub(crate) async fn document(response: reqwest::Response) -> Result<serde_json::Value, McpError> {
    if !response.status().is_success() {
        return Err(McpError::OAuth(format!(
            "OAuth metadata request returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| McpError::OAuth("Could not read OAuth metadata".into()))?
    {
        if bytes.len() + chunk.len() > MAX_DOCUMENT_BYTES {
            return Err(McpError::OAuth("OAuth metadata exceeds the size limit".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| McpError::OAuth("Invalid OAuth metadata document".into()))
}

async fn fetch(client: &Client, url: &str) -> Result<Option<serde_json::Value>, McpError> {
    validate_url(url)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| McpError::OAuth("Could not fetch OAuth metadata".into()))?;
    if matches!(response.status().as_u16(), 404 | 405) {
        return Ok(None);
    }
    document(response).await.map(Some)
}

fn well_known(issuer: &Url, suffix: &str, insert_before_path: bool) -> String {
    let mut url = issuer.clone();
    let path = issuer.path().trim_end_matches('/');
    url.set_path(&if insert_before_path {
        format!("/.well-known/{suffix}{path}")
    } else {
        format!("{path}/.well-known/{suffix}")
    });
    url.to_string()
}

/// Parse auth parameters without treating commas inside a quoted URI as delimiters.
fn resource_hint(header: &str) -> Option<String> {
    let mut quoted = false;
    let mut escaped = false;
    let mut start = 0;
    for (index, ch) in header.char_indices().chain(std::iter::once((header.len(), ','))) {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
        }
        if ch == ',' && !quoted {
            let part = header[start..index].trim();
            if let Some((key, value)) = part.split_once('=')
                && key
                    .split_whitespace()
                    .last()
                    .is_some_and(|name| name.eq_ignore_ascii_case("resource_metadata"))
            {
                let value = value.trim();
                return Some(value.strip_prefix('"')?.strip_suffix('"')?.to_string());
            }
            start = index + 1;
        }
    }
    None
}

async fn authorization_metadata(client: &Client, issuer: &str, resource: &str) -> Result<Metadata, McpError> {
    let issuer_url = validate_url(issuer)?;
    let candidates = [
        well_known(&issuer_url, "oauth-authorization-server", true),
        well_known(&issuer_url, "openid-configuration", false),
    ];
    for candidate in candidates {
        if let Some(value) = fetch(client, &candidate).await? {
            let mut metadata: Metadata = serde_json::from_value(value)
                .map_err(|_| McpError::OAuth("Invalid authorization server metadata".into()))?;
            if metadata.issuer.as_deref() != Some(issuer) {
                return Err(McpError::OAuth("Authorization server issuer mismatch".into()));
            }
            validate_metadata(&metadata)?;
            metadata.resource = resource.to_string();
            return Ok(metadata);
        }
    }
    Err(McpError::OAuth("Authorization server metadata is unavailable".into()))
}

fn validate_metadata(metadata: &Metadata) -> Result<(), McpError> {
    validate_url(&metadata.authorization_endpoint)?;
    validate_url(&metadata.token_endpoint)?;
    if let Some(endpoint) = &metadata.registration_endpoint {
        validate_url(endpoint)?;
    }
    if metadata
        .code_challenge_methods_supported
        .as_ref()
        .is_some_and(|methods| !methods.iter().any(|method| method == "S256"))
    {
        return Err(McpError::OAuth(
            "Authorization server does not support PKCE S256".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn discover(
    client: &Client,
    resource: &str,
    preferred_issuer: Option<&str>,
) -> Result<Metadata, McpError> {
    let resource_url = validate_url(resource)?;
    let mut candidates = Vec::new();
    // A GET is never followed as a redirect and carries no bearer credential.
    if let Ok(response) = client.get(resource).send().await
        && response.status() == reqwest::StatusCode::UNAUTHORIZED
    {
        for header in response.headers().get_all(reqwest::header::WWW_AUTHENTICATE) {
            if let Ok(header) = header.to_str()
                && let Some(hint) = resource_hint(header)
            {
                candidates.push(hint);
            }
        }
    }
    candidates.push(well_known(&resource_url, "oauth-protected-resource", true));
    let mut root = resource_url.clone();
    root.set_path("");
    root.set_query(None);
    let fallback = well_known(&root, "oauth-protected-resource", true);
    if !candidates.contains(&fallback) {
        candidates.push(fallback);
    }
    for candidate in candidates {
        if let Some(value) = fetch(client, &candidate).await? {
            let metadata: ResourceMetadata = serde_json::from_value(value)
                .map_err(|_| McpError::OAuth("Invalid protected resource metadata".into()))?;
            if metadata.resource != resource {
                return Err(McpError::OAuth(
                    "Protected resource metadata does not match this MCP server".into(),
                ));
            }
            let issuer = match preferred_issuer {
                Some(preferred) => metadata
                    .authorization_servers
                    .iter()
                    .find(|issuer| issuer.as_str() == preferred),
                None => metadata.authorization_servers.first(),
            }
            .ok_or_else(|| McpError::OAuth("No matching authorization server is advertised".into()))?;
            return authorization_metadata(client, issuer, resource).await;
        }
    }
    // Compatibility with older MCP servers; malformed standard metadata never
    // reaches this fallback. No DCR registration occurs during status/refresh.
    let base = resource.trim_end_matches('/');
    for suffix in ["oauth-authorization-server", "openid-configuration"] {
        if let Some(value) = fetch(client, &format!("{base}/.well-known/{suffix}")).await? {
            let mut metadata: Metadata =
                serde_json::from_value(value).map_err(|_| McpError::OAuth("Invalid legacy OAuth metadata".into()))?;
            validate_metadata(&metadata)?;
            if let Some(preferred) = preferred_issuer
                && metadata.issuer.as_deref() != Some(preferred)
            {
                return Err(McpError::OAuth(
                    "Authorization server changed; explicit login required".into(),
                ));
            }
            metadata.resource = resource.to_string();
            return Ok(metadata);
        }
    }
    Err(McpError::OAuth(
        "No OAuth protected resource or authorization server metadata found".into(),
    ))
}

pub(crate) async fn register(client: &Client, metadata: &Metadata, redirect_uri: &str) -> Result<ClientInfo, McpError> {
    let client_id = if let Some(endpoint) = &metadata.registration_endpoint {
        let response = client
            .post(endpoint)
            .json(&serde_json::json!({
                "client_name":"One Work", "redirect_uris":[redirect_uri],
                "grant_types":["authorization_code","refresh_token"], "response_types":["code"],
                "token_endpoint_auth_method":"none"
            }))
            .send()
            .await
            .map_err(|_| McpError::OAuth("Dynamic client registration request failed".into()))?;
        let value = document(response).await?;
        if value.get("client_secret").is_some_and(|value| !value.is_null())
            || value
                .get("token_endpoint_auth_method")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value != "none")
        {
            return Err(McpError::OAuth(
                "This MCP authorization server requires a confidential client; public PKCE registration was requested"
                    .into(),
            ));
        }
        value
            .get("client_id")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty() && value.len() <= 2048)
            .ok_or_else(|| McpError::OAuth("Registration returned no valid client ID".into()))?
            .to_owned()
    } else if metadata.issuer.is_none() {
        "one".to_owned()
    } else {
        return Err(McpError::OAuth("This authorization server does not offer dynamic registration; a provider-specific registered client is required".into()));
    };
    Ok(ClientInfo {
        client_id,
        issuer: metadata.issuer.clone(),
        resource: metadata.resource.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_aware_metadata_locations_and_challenges() {
        let url = Url::parse("https://example.com/tenant/mcp").unwrap();
        assert_eq!(
            well_known(&url, "oauth-protected-resource", true),
            "https://example.com/.well-known/oauth-protected-resource/tenant/mcp"
        );
        assert_eq!(
            well_known(&url, "openid-configuration", false),
            "https://example.com/tenant/mcp/.well-known/openid-configuration"
        );
        assert_eq!(
            resource_hint("Basic realm=\"a\", Bearer realm=\"b\", resource_metadata=\"https://example.com/meta?a,b\""),
            Some("https://example.com/meta?a,b".into())
        );
        assert!(resource_hint("Bearer realm=\"resource_metadata=foo\"").is_none());
    }
    #[test]
    fn rejects_insecure_and_credential_bearing_endpoints() {
        for url in [
            "http://example.com/mcp",
            "https://user:password@example.com/mcp",
            "https://example.com/mcp#fragment",
            "file:///tmp/oauth",
        ] {
            assert!(validate_url(url).is_err());
        }
        for url in [
            "http://127.0.0.1:1234/mcp",
            "http://[::1]:1234/mcp",
            "https://example.com/mcp",
        ] {
            assert!(validate_url(url).is_ok());
        }
    }
}
