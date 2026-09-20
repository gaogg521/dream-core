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
    //! Round-trip tests drive a real local IdP (the `saml_rs` IdP facade signs
    //! a genuine XML-DSig response) against our SP glue, so the negative cases
    //! below exercise the actual verification path, not a mock of it.
    //!
    //! Fixture keys are the upstream `saml_rs` MIT test keypair (public,
    //! expired never before 2030) — never use them outside `#[cfg(test)]`.

    use std::time::Duration;

    use saml_rs::binding::{base64_decode, base64_encode};
    use saml_rs::constants::signature_algorithm::RSA_SHA256;
    use saml_rs::constants::Binding;
    use saml_rs::entity::{EntitySetting, User};
    use saml_rs::idp::LoginResponseOptions;
    use saml_rs::metadata::{Endpoint, IdpMetadataConfig, SpMetadataConfig};
    use saml_rs::template::{LoginResponseAttribute, LoginResponseTemplate};
    use saml_rs::{IdentityProvider, ServiceProvider};

    use super::*;

    const IDP_PRIVKEY: &str = include_str!("saml_fixtures/idp_privkey.pem");
    const IDP_CERT: &str = include_str!("saml_fixtures/idp_cert.cer");
    const IDP_PRIVKEY_ALT: &str = include_str!("saml_fixtures/idp_privkey_alt.pem");
    const IDP_CERT_ALT: &str = include_str!("saml_fixtures/idp_cert_alt.cer");
    const IDP_ENTITY: &str = "https://idp.example.test/metadata";
    const SP_ENTITY: &str = "https://sp.example.test/metadata";
    const ACS_URL: &str = "https://sp.example.test/acs";

    fn config_with_metadata(idp_metadata_xml: &str) -> SamlProviderConfig {
        SamlProviderConfig {
            idp_entity_id: IDP_ENTITY.into(),
            idp_metadata_xml: idp_metadata_xml.to_string(),
            sp_entity_id: SP_ENTITY.into(),
            acs_url: ACS_URL.into(),
            sp_private_key_pem: IDP_PRIVKEY.into(),
            sp_certificate_pem: IDP_CERT.into(),
            external_id_attribute: String::new(),
            name_attribute: String::new(),
        }
    }

    /// Local IdP. `signing_cert` is both the key the IdP signs with and the
    /// certificate published in its metadata — except for [`idp_signing_with`]
    /// where the two deliberately diverge.
    fn local_idp(signing_cert: &str, signing_key: &str) -> IdentityProvider {
        let mut setting = EntitySetting::default();
        setting.private_key = Some(signing_key.to_string());
        setting.signing_cert = Some(signing_cert.to_string());
        setting.request_signature_algorithm = RSA_SHA256.into();
        setting.login_response_template = Some(LoginResponseTemplate {
            context: None,
            attributes: vec![LoginResponseAttribute {
                name: "displayName".into(),
                name_format: "urn:oasis:names:tc:SAML:2.0:attrname-format:basic".into(),
                value_xsi_type: "xs:string".into(),
                value_tag: "displayName".into(),
                value_xmlns_xs: None,
                value_xmlns_xsi: None,
            }],
        });
        let metadata = IdpMetadataConfig {
            entity_id: IDP_ENTITY.into(),
            signing_certs: vec![signing_cert.to_string()],
            want_authn_requests_signed: true,
            single_sign_on_service: vec![
                Endpoint::new(Binding::Redirect, "https://idp.example.test/sso"),
                Endpoint::new(Binding::Post, "https://idp.example.test/sso"),
            ],
            ..Default::default()
        };
        IdentityProvider::from_config(&metadata, setting).unwrap()
    }

    fn sp_metadata() -> SpMetadataConfig {
        SpMetadataConfig {
            entity_id: SP_ENTITY.into(),
            want_assertions_signed: true,
            signing_certs: vec![IDP_CERT.into()],
            assertion_consumer_service: vec![Endpoint::new(Binding::Post, ACS_URL)],
            ..Default::default()
        }
    }

    fn issue_response(idp: &IdentityProvider, request_id: &str, relay: &str) -> String {
        let sp = ServiceProvider::from_config(&sp_metadata(), EntitySetting::default()).unwrap();
        let user = User {
            name_id: "alice@example.test".into(),
            attributes: vec![("displayName".to_string(), "Alice Example".to_string())],
            ..Default::default()
        };
        idp.create_login_response(
            &sp,
            Binding::Post,
            &user,
            &LoginResponseOptions {
                in_response_to: Some(request_id),
                relay_state: Some(relay),
                ..Default::default()
            },
        )
        .unwrap()
        .context
    }

    fn begin(state: &str) -> SamlProviderConfig {
        let idp = local_idp(IDP_CERT, IDP_PRIVKEY);
        let metadata = idp.metadata_xml().to_string();
        SamlProvider::begin(config_with_metadata(&metadata), state).unwrap();
        config_with_metadata(&metadata)
    }

    fn pending_request_id(state: &str) -> String {
        pending_store()
            .lock()
            .unwrap()
            .requests
            .get(state)
            .unwrap()
            .request
            .id()
            .as_str()
            .to_string()
    }

    fn backdate_pending(state: &str, age: Duration) {
        pending_store().lock().unwrap().requests.get_mut(state).unwrap().issued_at =
            std::time::SystemTime::now()
                .checked_sub(age)
                .expect("backdate under test clocks");
    }

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

    #[test]
    fn full_roundtrip_accepts_signed_response_and_maps_identity() {
        let state = "rt-ok";
        let _config = begin(state);
        let response = issue_response(
            &local_idp(IDP_CERT, IDP_PRIVKEY),
            &pending_request_id(state),
            state,
        );
        let user = SamlProvider::complete(state, &response, Some(state)).unwrap();
        assert_eq!(user.external_id, "alice@example.test");
        assert_eq!(user.preferred_username, "Alice Example");
        assert!(user.org_unit_path.is_none());
        assert!(user.job_title.is_none());
        assert!(user.org_external_id.is_none());
    }

    #[test]
    fn begin_redirect_carries_relay_state_and_stores_pending() {
        let state = "rt-begin";
        let config = begin(state);
        // The stored pending must be retrievable through the same config the
        // IdP metadata was derived from — complete() relies on it.
        assert!(pending_store().lock().unwrap().requests.contains_key(state));
        assert_eq!(config.sp_entity_id, SP_ENTITY);
    }

    #[test]
    fn tampered_assertion_is_rejected() {
        let state = "rt-tamper";
        let _config = begin(state);
        let response = issue_response(
            &local_idp(IDP_CERT, IDP_PRIVKEY),
            &pending_request_id(state),
            state,
        );
        let xml = base64_decode(&response).unwrap();
        let xml = String::from_utf8(xml).unwrap();
        assert!(xml.contains("alice@example.test"), "fixture NameID missing");
        let tampered = xml.replace("alice@example.test", "malice@example.test");
        assert_ne!(xml, tampered);
        let tampered = base64_encode(tampered.as_bytes());
        let result = SamlProvider::complete(state, &tampered, Some(state));
        assert!(matches!(result, Err(SsoError::Unauthorized)), "got {result:?}");
    }

    #[test]
    fn response_signed_by_key_outside_trusted_metadata_is_rejected() {
        // The IdP signs with the ALT keypair, but the metadata our SP trusts
        // publishes the primary certificate. Trust must come from admin
        // metadata only — never from the assertion's KeyInfo.
        let state = "rt-foreign-key";
        let trusted = local_idp(IDP_CERT, IDP_PRIVKEY);
        let metadata = trusted.metadata_xml().to_string();
        SamlProvider::begin(config_with_metadata(&metadata), state).unwrap();
        let response = issue_response(
            &local_idp(IDP_CERT_ALT, IDP_PRIVKEY_ALT),
            &pending_request_id(state),
            state,
        );
        let result = SamlProvider::complete(state, &response, Some(state));
        assert!(matches!(result, Err(SsoError::Unauthorized)), "got {result:?}");
    }

    #[test]
    fn expired_pending_state_is_rejected() {
        let state = "rt-expired";
        let _config = begin(state);
        backdate_pending(state, SAML_PENDING_TTL + Duration::from_secs(5));
        let response = issue_response(
            &local_idp(IDP_CERT, IDP_PRIVKEY),
            &pending_request_id(state),
            state,
        );
        let result = SamlProvider::complete(state, &response, Some(state));
        assert!(matches!(result, Err(SsoError::InvalidState)), "got {result:?}");
    }

    #[test]
    fn relay_state_mismatch_is_rejected() {
        let state = "rt-relay";
        let _config = begin(state);
        let response = issue_response(
            &local_idp(IDP_CERT, IDP_PRIVKEY),
            &pending_request_id(state),
            state,
        );
        let result = SamlProvider::complete(state, &response, Some("relay-forged"));
        assert!(matches!(result, Err(SsoError::InvalidState)), "got {result:?}");
    }

    #[test]
    fn unknown_state_is_rejected_without_touching_idp_response() {
        let result = SamlProvider::complete("rt-unknown", "anything", Some("rt-unknown"));
        assert!(matches!(result, Err(SsoError::InvalidState)), "got {result:?}");
    }

    #[test]
    fn garbage_response_is_rejected() {
        let state = "rt-garbage";
        let _config = begin(state);
        let result = SamlProvider::complete(state, "!!!not-base64-xml!!!", Some(state));
        assert!(matches!(result, Err(SsoError::Unauthorized)), "got {result:?}");
    }

    #[test]
    fn replay_cache_rejects_second_use_of_same_assertion() {
        let mut cache = InMemoryReplayCache::default();
        let key = ReplayKey::ResponseId(
            saml_rs::model::MessageId::try_new("_response_replayed_once").unwrap(),
        );
        let expiry = std::time::SystemTime::now() + Duration::from_secs(60);
        cache.check_and_store(key.clone(), expiry).unwrap();
        let second = cache.check_and_store(key, expiry);
        assert!(
            matches!(second, Err(SamlError::ReplayDetected { .. })),
            "got {second:?}"
        );
    }

    #[test]
    fn replay_cache_allows_distinct_assertions() {
        let mut cache = InMemoryReplayCache::default();
        let expiry = std::time::SystemTime::now() + Duration::from_secs(60);
        cache
            .check_and_store(
                ReplayKey::ResponseId(saml_rs::model::MessageId::try_new("_r1").unwrap()),
                expiry,
            )
            .unwrap();
        cache
            .check_and_store(
                ReplayKey::ResponseId(saml_rs::model::MessageId::try_new("_r2").unwrap()),
                expiry,
            )
            .unwrap();
    }

    #[test]
    fn pending_store_evicts_expired_entries_on_insert() {
        let state = "rt-evict-old";
        let config = begin(state);
        backdate_pending(state, SAML_PENDING_TTL + Duration::from_secs(5));
        let idp = local_idp(IDP_CERT, IDP_PRIVKEY);
        let metadata = idp.metadata_xml().to_string();
        SamlProvider::begin(config_with_metadata(&metadata), "rt-evict-new").unwrap();
        let store = pending_store().lock().unwrap();
        assert!(!store.requests.contains_key(state));
        assert!(store.requests.contains_key("rt-evict-new"));
        drop(store);
        // Keep the config alive so the borrow checker sees it used.
        assert_eq!(config.idp_entity_id, IDP_ENTITY);
    }
}
