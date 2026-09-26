//! OIDC `id_token` verification against the IdP JWKS.
//!
//! Identity still comes from userinfo (the access token was exchanged over TLS
//! with the client secret). Verifying the `id_token` is defense in depth: it
//! binds the code-exchange result to the same subject the userinfo endpoint
//! returns, and it rejects tokens the IdP did not sign.
//!
//! ## JWKS endpoint down (explicit fail-closed choice)
//! If `jwks_uri` is missing from discovery, or the JWKS HTTP call fails and we
//! have no cached key for the token's `kid`, we **refuse the login**. We never
//! skip signature verification to keep SSO working. A still-fresh cache entry
//! may satisfy a known `kid` while the endpoint is briefly down; an unknown
//! `kid` always triggers one refresh, then fails if the key is still absent
//! (IdP key rotation).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;

use crate::error::SsoError;
use crate::providers::oidc::{OidcDiscovery, OidcProviderConfig};

const JWKS_TTL: Duration = Duration::from_secs(60 * 60);
const ALLOWED_ALGS: &[Algorithm] = &[
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
];

#[derive(Debug, Clone)]
struct CachedJwks {
    set: JwkSet,
    fetched_at: Instant,
}

/// Process- or test-scoped JWKS cache, keyed by `jwks_uri`.
pub struct JwksCache {
    inner: Mutex<HashMap<String, CachedJwks>>,
}

impl JwksCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn process_global() -> &'static JwksCache {
        static CACHE: std::sync::OnceLock<JwksCache> = std::sync::OnceLock::new();
        CACHE.get_or_init(JwksCache::new)
    }

    fn cached(&self, uri: &str) -> Option<JwkSet> {
        let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.get(uri).and_then(|c| {
            (c.fetched_at.elapsed() < JWKS_TTL).then(|| c.set.clone())
        })
    }

    fn find_in(set: &JwkSet, kid: Option<&str>) -> Option<Jwk> {
        if let Some(kid) = kid.filter(|k| !k.is_empty()) {
            return set.find(kid).cloned();
        }
        if set.keys.len() == 1 {
            return set.keys.first().cloned();
        }
        None
    }

    async fn fetch(&self, client: &reqwest::Client, jwks_uri: &str) -> Result<JwkSet, SsoError> {
        let resp = client.get(jwks_uri).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(SsoError::Internal(format!("OIDC JWKS fetch failed: HTTP {status}")));
        }
        let set: JwkSet = resp
            .json()
            .await
            .map_err(|e| SsoError::Internal(format!("OIDC JWKS parse: {e}")))?;
        if set.keys.is_empty() {
            return Err(SsoError::Internal("OIDC JWKS document contains no keys".into()));
        }
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(
            jwks_uri.to_owned(),
            CachedJwks {
                set: set.clone(),
                fetched_at: Instant::now(),
            },
        );
        Ok(set)
    }

    /// Resolve a JWK for `kid`. Cache hit first; on miss, one forced refresh
    /// (covers IdP key rotation), then reject if still absent.
    pub async fn key_for_kid(
        &self,
        client: &reqwest::Client,
        jwks_uri: &str,
        kid: Option<&str>,
    ) -> Result<Jwk, SsoError> {
        if let Some(set) = self.cached(jwks_uri)
            && let Some(jwk) = Self::find_in(&set, kid)
        {
            return Ok(jwk);
        }
        let set = self.fetch(client, jwks_uri).await?;
        Self::find_in(&set, kid).ok_or_else(|| {
            SsoError::Internal(format!(
                "OIDC id_token kid {:?} not present in JWKS after refresh",
                kid.unwrap_or("")
            ))
        })
    }
}

impl Default for JwksCache {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct IdTokenClaims {
    sub: String,
    #[serde(default)]
    nonce: Option<String>,
    /// Catch-all so we can compare the configured external-id claim.
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl IdTokenClaims {
    fn claim(&self, name: &str) -> Option<String> {
        if name == "sub" {
            return Some(self.sub.clone());
        }
        self.extra.get(name).and_then(super::oidc::claim_as_string)
    }
}

/// Header `alg` must be an asymmetric algorithm we verify against JWKS.
/// `alg: none` and HMAC (shared-secret) algorithms are rejected before any
/// signature check — JWT's classic pitfalls.
fn reject_insecure_alg(token: &str) -> Result<jsonwebtoken::Header, SsoError> {
    if header_alg_is_none(token) {
        return Err(SsoError::Internal("OIDC id_token rejected: alg none".into()));
    }
    let header = decode_header(token).map_err(|e| SsoError::Internal(format!("OIDC id_token header: {e}")))?;
    if !ALLOWED_ALGS.contains(&header.alg) {
        return Err(SsoError::Internal(format!(
            "OIDC id_token rejected: unsupported alg {:?}",
            header.alg
        )));
    }
    Ok(header)
}

fn header_alg_is_none(token: &str) -> bool {
    let Some(first) = token.split('.').next() else {
        return false;
    };
    let Ok(bytes) = decode_b64url(first) else {
        return false;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    v.get("alg")
        .and_then(|a| a.as_str())
        .is_some_and(|a| a.eq_ignore_ascii_case("none") || a.is_empty())
}

fn decode_b64url(s: &str) -> Result<Vec<u8>, ()> {
    use base64::Engine as _;
    let mut padded = s.replace('-', "+").replace('_', "/");
    while !padded.len().is_multiple_of(4) {
        padded.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .map_err(|_| ())
}

pub(crate) async fn verify_id_token(
    client: &reqwest::Client,
    cache: &JwksCache,
    discovery: &OidcDiscovery,
    config: &OidcProviderConfig,
    id_token: &str,
    nonce: &str,
) -> Result<IdTokenClaims, SsoError> {
    let jwks_uri = discovery
        .jwks_uri
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| SsoError::Internal("OIDC discovery missing jwks_uri".into()))?;

    let header = reject_insecure_alg(id_token)?;
    let kid = header.kid.as_deref();
    let jwk = cache.key_for_kid(client, jwks_uri, kid).await?;
    let decoding_key =
        DecodingKey::from_jwk(&jwk).map_err(|e| SsoError::Internal(format!("OIDC JWKS key: {e}")))?;

    let mut validation = Validation::new(header.alg);
    // Only the token's own algorithm. Mixing RSA and EC in `algorithms`
    // makes jsonwebtoken 10 reject the key as InvalidAlgorithm.
    validation.set_issuer(&[config.issuer.as_str()]);
    validation.set_audience(&[config.client_id.as_str()]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.validate_exp = true;
    validation.leeway = 60;

    let data = decode::<IdTokenClaims>(id_token, &decoding_key, &validation)
        .map_err(|e| SsoError::Internal(format!("OIDC id_token verify: {e}")))?;

    let expected_nonce = nonce.trim();
    if expected_nonce.is_empty() {
        return Err(SsoError::Internal("OIDC nonce missing from login state".into()));
    }
    match data.claims.nonce.as_deref().map(str::trim) {
        Some(got) if got == expected_nonce => {}
        Some(_) => return Err(SsoError::Internal("OIDC id_token nonce mismatch".into())),
        None => return Err(SsoError::Internal("OIDC id_token missing nonce".into())),
    }
    Ok(data.claims)
}

/// Userinfo and id_token must name the same person. We do not pick one.
pub(crate) fn identities_must_match(
    id_token: &IdTokenClaims,
    userinfo: &serde_json::Value,
    external_id_claim: &str,
) -> Result<(), SsoError> {
    let token_sub = id_token.sub.trim();
    let info_sub = userinfo
        .get("sub")
        .and_then(super::oidc::claim_as_string)
        .unwrap_or_default();
    if info_sub.is_empty() || token_sub != info_sub.trim() {
        return Err(SsoError::Internal(
            "OIDC id_token sub does not match userinfo sub".into(),
        ));
    }
    if external_id_claim != "sub" {
        let from_token = id_token.claim(external_id_claim).unwrap_or_default();
        let from_info = userinfo
            .get(external_id_claim)
            .and_then(super::oidc::claim_as_string)
            .unwrap_or_default();
        if from_token.is_empty() || from_token != from_info {
            return Err(SsoError::Internal(format!(
                "OIDC id_token {external_id_claim} does not match userinfo"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_header_alg_is_none(token: &str) -> bool {
    header_alg_is_none(token)
}
