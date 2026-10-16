//! Configuration and purpose-bound capabilities for intranet Office editing.
//! Routes must still recheck the actor and document ACL at each use.
use crate::DevopsError;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct OfficeConfig {
    pub public_url: Url,
    pub internal_url: Url,
    pub callback_base: Url,
    secret: Vec<u8>,
}

fn base_url(value: &str) -> Result<Url, DevopsError> {
    let mut url = Url::parse(value).map_err(|_| DevopsError::BadRequest("invalid Office service URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(DevopsError::BadRequest(
            "Office service requires a plain HTTP(S) base URL".into(),
        ));
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

impl OfficeConfig {
    pub fn new(public_url: &str, internal_url: &str, callback_base: &str, secret: &[u8]) -> Result<Self, DevopsError> {
        if secret.len() < 32 {
            return Err(DevopsError::BadRequest(
                "Office JWT secret must contain at least 32 bytes".into(),
            ));
        }
        Ok(Self {
            public_url: base_url(public_url)?,
            internal_url: base_url(internal_url)?,
            callback_base: base_url(callback_base)?,
            secret: secret.to_vec(),
        })
    }

    pub fn sign_editor_config(&self, config: &serde_json::Value) -> Result<String, DevopsError> {
        jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            config,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|_| DevopsError::Internal("could not sign Office editor configuration".into()))
    }

    /// Never expose the engine JWT secret or use an editor-config JWT as a file ticket.
    fn capability_key(&self) -> Vec<u8> {
        Sha256::new()
            .chain_update(b"onework-knowledge-office-capability-v1\0")
            .chain_update(&self.secret)
            .finalize()
            .to_vec()
    }

    pub fn ticket(&self, claims: &OfficeTicket) -> Result<String, DevopsError> {
        claims.validate()?;
        jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            claims,
            &EncodingKey::from_secret(&self.capability_key()),
        )
        .map_err(|_| DevopsError::Internal("could not sign Office file capability".into()))
    }

    pub fn verify_ticket(&self, token: &str, purpose: &str) -> Result<OfficeTicket, DevopsError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = 0;
        validation.set_audience(&[purpose]);
        validation.set_issuer(&["onework-knowledge-office"]);
        let claims =
            jsonwebtoken::decode::<OfficeTicket>(token, &DecodingKey::from_secret(&self.capability_key()), &validation)
                .map_err(|_| DevopsError::Forbidden("invalid or expired Office file capability".into()))?
                .claims;
        claims.validate()?;
        Ok(claims)
    }

    /// Read only signed engine fields. An unsigned outer body cannot override them.
    pub fn verify_callback(&self, token: &str) -> Result<serde_json::Value, DevopsError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        validation.validate_aud = false;
        let value =
            jsonwebtoken::decode::<serde_json::Value>(token, &DecodingKey::from_secret(&self.secret), &validation)
                .map_err(|_| DevopsError::Forbidden("invalid Office callback signature".into()))?
                .claims;
        Ok(value.get("payload").cloned().unwrap_or(value))
    }

    /// A signed download URL is still restricted to the configured document server.
    /// The caller must use a no-redirect HTTP client and a bounded response body.
    pub fn callback_download_url(&self, value: &str) -> Result<Url, DevopsError> {
        let url =
            Url::parse(value).map_err(|_| DevopsError::BadRequest("invalid Office callback download URL".into()))?;
        let public_match = url.origin() == self.public_url.origin();
        let internal_match = url.origin() == self.internal_url.origin();
        if (!public_match && !internal_match)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(DevopsError::Forbidden(
                "Office callback download origin is not configured".into(),
            ));
        }
        let base = if public_match {
            &self.public_url
        } else {
            &self.internal_url
        };
        let relative = url
            .path()
            .strip_prefix(base.path())
            .filter(|path| path.starts_with("cache/files/"))
            .ok_or_else(|| DevopsError::Forbidden("Office callback must reference its document cache".into()))?;
        let mut result = self
            .internal_url
            .join(relative)
            .map_err(|_| DevopsError::BadRequest("invalid Office cache path".into()))?;
        result.set_query(url.query());
        Ok(result)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OfficeTicket {
    pub iss: String,
    pub aud: String,
    pub exp: u64,
    pub actor: String,
    pub document: String,
    pub session: String,
    pub source: String,
}
impl OfficeTicket {
    fn validate(&self) -> Result<(), DevopsError> {
        if self.iss != "onework-knowledge-office"
            || !["source", "callback"].contains(&self.aud.as_str())
            || self.actor.is_empty()
            || self.actor.len() > 128
            || self.document.is_empty()
            || self.document.len() > 128
            || uuid::Uuid::parse_str(&self.session).is_err()
            || uuid::Uuid::parse_str(&self.source).is_err()
        {
            return Err(DevopsError::Forbidden("invalid Office capability scope".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> OfficeConfig {
        OfficeConfig::new(
            "https://office.internal/",
            "http://office:80/",
            "http://onework/",
            b"0123456789abcdef0123456789abcdef",
        )
        .unwrap()
    }
    fn claims() -> OfficeTicket {
        OfficeTicket {
            iss: "onework-knowledge-office".into(),
            aud: "source".into(),
            exp: (dream_core_common::now_ms() / 1000) as u64 + 60,
            actor: "owner".into(),
            document: "document".into(),
            session: uuid::Uuid::new_v4().to_string(),
            source: uuid::Uuid::new_v4().to_string(),
        }
    }
    #[test]
    fn file_tickets_are_expiring_purpose_bound_and_distinct_from_editor_tokens() {
        let config = config();
        let mut claims = claims();
        let token = config.ticket(&claims).unwrap();
        assert_eq!(config.verify_ticket(&token, "source").unwrap().actor, "owner");
        assert!(config.verify_ticket(&token, "callback").is_err());
        claims.exp = 1;
        assert!(
            config
                .verify_ticket(&config.ticket(&claims).unwrap(), "source")
                .is_err()
        );
        let forged = config
            .sign_editor_config(&serde_json::to_value(claims).unwrap())
            .unwrap();
        assert!(config.verify_ticket(&forged, "source").is_err());
    }
    #[test]
    fn callback_downloads_are_confined_to_the_configured_engine_cache() {
        let config = config();
        assert_eq!(
            config
                .callback_download_url("https://office.internal/cache/files/key/output.docx?signature=abc")
                .unwrap()
                .as_str(),
            "http://office/cache/files/key/output.docx?signature=abc"
        );
        for value in [
            "http://other/cache/files/a",
            "http://office.internal/cache/files/a",
            "https://office.internal/private",
            "https://user@office.internal/cache/files/a",
            "https://office.internal/cache/files/../admin",
            "https://office.internal/cache/files/a#fragment",
        ] {
            assert!(config.callback_download_url(value).is_err(), "{value}");
        }
    }
    #[test]
    fn callback_signature_is_checked_and_only_signed_payload_is_returned() {
        let config = config();
        let token = config
            .sign_editor_config(&serde_json::json!({"payload": {"key":"k", "status":2}}))
            .unwrap();
        assert_eq!(config.verify_callback(&token).unwrap()["status"], 2);
        assert!(config.verify_callback("unsigned").is_err());
    }
    #[test]
    fn rejects_credentials_non_http_urls_and_short_secrets() {
        for url in [
            "file:///tmp/office",
            "https://user:pass@office/",
            "https://office/?token=x",
            "https://office/#fragment",
        ] {
            assert!(
                OfficeConfig::new(
                    url,
                    "http://office/",
                    "http://core/",
                    b"0123456789abcdef0123456789abcdef"
                )
                .is_err()
            );
        }
        assert!(OfficeConfig::new("http://office/", "http://office/", "http://core/", b"weak").is_err());
    }
}
