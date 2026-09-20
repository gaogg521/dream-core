//! SAML 2.0 service-provider flow.
//!
//! `saml-rs` owns XML parsing, C14N and XML-DSig validation.  This module
//! deliberately never parses assertion XML itself: the identity is read only
//! from the library's verified SSO session, which also enforces issuer,
//! audience, destination, recipient, `InResponseTo` and time-window checks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use saml_rs::{
    AcsEndpoint, AuthnRequestSigningPolicy, BrowserInput, CertificatePem, Credentials, EntityId, FormField,
    IdpDescriptor, MetadataTrustPolicy, PendingAuthnRequest, PrivateKeyPem, ReplayCache, ReplayKey, ReplayPolicy, Saml,
    SamlError, SamlValidationContext, SpConfig, SpValidationPolicy, SsoResponse, StartSso,
};
use serde::Deserialize;

use crate::error::SsoError;
use crate::providers::ProviderUserInfo;

const SAML_PENDING_TTL: Duration = Duration::from_secs(10 * 60);
const REPLAY_RETENTION: Duration = Duration::from_secs(10 * 60);

/// Admin-provided configuration. Metadata is treated as an explicitly trusted
/// administrator input; the IdP signing keys are taken only from it, never
/// from an untrusted assertion `KeyInfo`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamlProviderConfig {
    pub idp_entity_id: String,
    pub idp_metadata_xml: String,
    pub sp_entity_id: String,
    pub acs_url: String,
    pub sp_private_key_pem: String,
    pub sp_certificate_pem: String,
    #[serde(default)]
    pub external_id_attribute: String,
    #[serde(default)]
    pub name_attribute: String,
}

impl SamlProviderConfig {
    pub fn external_id_attribute_or_default(&self) -> &str {
        if self.external_id_attribute.trim().is_empty() {
            "NameID"
        } else {
            self.external_id_attribute.as_str()
        }
    }

    pub fn name_attribute_or_default(&self) -> &str {
        if self.name_attribute.trim().is_empty() {
            "displayName"
        } else {
            self.name_attribute.as_str()
        }
    }
}

struct Pending {
    config: SamlProviderConfig,
    request: PendingAuthnRequest,
    issued_at: SystemTime,
}

#[derive(Default)]
struct PendingStore {
    requests: HashMap<String, Pending>,
}

impl PendingStore {
    fn insert(&mut self, state: String, pending: Pending) {
        self.requests.retain(|_, value| {
            value
                .issued_at
                .elapsed()
                .map(|age| age < SAML_PENDING_TTL)
                .unwrap_or(false)
        });
        self.requests.insert(state, pending);
    }

    fn take(&mut self, state: &str) -> Option<Pending> {
        self.requests.remove(state)
    }
}

#[derive(Default)]
struct InMemoryReplayCache {
    seen: Arc<Mutex<HashMap<String, SystemTime>>>,
}

impl ReplayCache for InMemoryReplayCache {
    fn check_and_store(&mut self, key: ReplayKey, expires_at: SystemTime) -> Result<(), SamlError> {
        let key = format!("{key:?}");
        let now = SystemTime::now();
        let mut seen = self
            .seen
            .lock()
            .map_err(|_| SamlError::ReplayDetected { key: key.clone() })?;
        seen.retain(|_, expiry| *expiry > now);
        if seen.contains_key(&key) {
            return Err(SamlError::ReplayDetected { key });
        }
        seen.insert(key, expires_at);
        Ok(())
    }
}

fn pending_store() -> &'static Mutex<PendingStore> {
    static STORE: OnceLock<Mutex<PendingStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(PendingStore::default()))
}

fn replay_cache() -> &'static Arc<Mutex<HashMap<String, SystemTime>>> {
    static CACHE: OnceLock<Arc<Mutex<HashMap<String, SystemTime>>>> = OnceLock::new();
    CACHE.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn service_provider(config: &SamlProviderConfig) -> Result<Saml<saml_rs::Sp>, SsoError> {
    let entity_id = EntityId::try_new(config.sp_entity_id.as_str())
        .map_err(|e| SsoError::BadRequest(format!("SAML SP entityId: {e}")))?;
    let acs =
        AcsEndpoint::post(config.acs_url.as_str()).map_err(|e| SsoError::BadRequest(format!("SAML ACS URL: {e}")))?;
    let credentials = Credentials {
        signing_key: Some(PrivateKeyPem::new(config.sp_private_key_pem.as_str())),
        signing_certificate: Some(CertificatePem::new(config.sp_certificate_pem.as_str())),
        ..Credentials::default()
    };
    let mut validation = SpValidationPolicy::strict();
    // Strictly verify incoming assertions/responses. Signed AuthnRequests are
    // required because this configuration requires the administrator to supply
    // the SP private key and certificate.
    validation.authn_requests = AuthnRequestSigningPolicy::Sign;
    let config = SpConfig::builder(entity_id)
        .acs_endpoint(acs)
        .credentials(credentials)
        .validation(validation)
        .build()
        .map_err(|e| SsoError::BadRequest(format!("SAML SP configuration: {e}")))?;
    Saml::sp(config).map_err(|e| SsoError::Internal(format!("SAML SP initialize: {e}")))
}

fn idp(config: &SamlProviderConfig) -> Result<IdpDescriptor, SsoError> {
    let idp_entity = EntityId::try_new(config.idp_entity_id.as_str())
        .map_err(|e| SsoError::BadRequest(format!("SAML IdP entityId: {e}")))?;
    IdpDescriptor::from_metadata_xml_for(
        idp_entity,
        config.idp_metadata_xml.as_str(),
        MetadataTrustPolicy::UnsignedForCompatibility,
    )
    .map_err(|e| SsoError::BadRequest(format!("SAML IdP metadata: {e}")))
}

pub struct SamlProvider;

impl SamlProvider {
    /// Build a redirect AuthnRequest and keep its typed correlation record
    /// server-side. The opaque OAuth state is the RelayState.
    pub fn begin(config: SamlProviderConfig, state: &str) -> Result<String, SsoError> {
        let provider = service_provider(&config)?;
        let idp = idp(&config)?;
        let relay = saml_rs::RelayStateParam::try_from_option(Some(state.to_owned()))
            .map_err(|e| SsoError::BadRequest(format!("SAML relay state: {e}")))?;
        let started = provider
            .start_sso(&idp, StartSso::redirect().relay_state(relay))
            .map_err(|e| SsoError::Internal(format!("SAML AuthnRequest: {e}")))?;
        let url = started
            .outbound
            .redirect_url()
            .map_err(|e| SsoError::Internal(format!("SAML redirect binding: {e}")))?
            .to_string();
        pending_store()
            .lock()
            .map_err(|_| SsoError::Internal("SAML pending state lock poisoned".into()))?
            .insert(
                state.to_owned(),
                Pending {
                    config,
                    request: started.pending,
                    issued_at: SystemTime::now(),
                },
            );
        Ok(url)
    }

    /// Validate the IdP POST response and only then map its verified identity.
    pub fn complete(state: &str, saml_response: &str, relay_state: Option<&str>) -> Result<ProviderUserInfo, SsoError> {
        if relay_state.map(str::trim).filter(|s| !s.is_empty()) != Some(state) {
            return Err(SsoError::InvalidState);
        }
        let pending = pending_store()
            .lock()
            .map_err(|_| SsoError::Internal("SAML pending state lock poisoned".into()))?
            .take(state)
            .ok_or(SsoError::InvalidState)?;
        if pending
            .issued_at
            .elapsed()
            .map(|age| age >= SAML_PENDING_TTL)
            .unwrap_or(true)
        {
            return Err(SsoError::InvalidState);
        }
        let provider = service_provider(&pending.config)?;
        let idp = idp(&pending.config)?;
        let fields = vec![
            FormField::new("SAMLResponse", saml_response.to_owned()),
            FormField::new("RelayState", state.to_owned()),
        ];
        let mut replay = InMemoryReplayCache {
            seen: replay_cache().clone(),
        };
        let validation = SamlValidationContext::new(SystemTime::now(), ReplayPolicy::RequireCache(&mut replay))
            .with_replay_retention(REPLAY_RETENTION);
        let session = provider
            .finish_sso(
                &idp,
                &pending.request,
                BrowserInput::<SsoResponse>::post(fields),
                validation,
            )
            .map_err(|_| SsoError::Unauthorized)?;
        let first_attribute = |name: &str| {
            session
                .attributes()
                .get(name)
                .and_then(|attribute| attribute.values().first())
                .map(|value| value.as_str().trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let external_id = if pending.config.external_id_attribute_or_default() == "NameID" {
            session.name_id().value().trim().to_owned()
        } else {
            first_attribute(pending.config.external_id_attribute_or_default()).ok_or(SsoError::IdentityMissing)?
        };
        if external_id.is_empty() {
            return Err(SsoError::IdentityMissing);
        }
        // `SsoSession` only exposes values from an authenticated assertion.
        // Presentation values must nevertheless be optional; NameID is the
        // stable fallback when the IdP does not issue a display-name attribute.
        let preferred_username =
            first_attribute(pending.config.name_attribute_or_default()).unwrap_or_else(|| external_id.clone());
        Ok(ProviderUserInfo {
            external_id,
            preferred_username,
            org_unit_path: None,
            job_title: None,
            org_external_id: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_identity_to_verified_name_id() {
        let cfg = SamlProviderConfig {
            idp_entity_id: "https://idp.example.test".into(),
            idp_metadata_xml: "<EntityDescriptor/>".into(),
            sp_entity_id: "https://sp.example.test".into(),
            acs_url: "https://sp.example.test/acs".into(),
            sp_private_key_pem: "key".into(),
            sp_certificate_pem: "cert".into(),
            external_id_attribute: String::new(),
            name_attribute: String::new(),
        };
        assert_eq!(cfg.external_id_attribute_or_default(), "NameID");
        assert_eq!(cfg.name_attribute_or_default(), "displayName");
    }
}
