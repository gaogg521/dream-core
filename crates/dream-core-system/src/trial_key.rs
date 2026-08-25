//! Requesting a capped-spend trial model key from the company's broker.
//!
//! dream-core never holds a vendor Management Key — that stays on a
//! separate, company-run broker service (`dream-trial-broker`). This module
//! only forwards this install's request to that broker and relays back
//! whatever it decides (issue a key, or refuse because this device already
//! claimed one / the broker is rate-limiting / today's spend budget is
//! spent). Wiring the returned key into a local `providers` row is the
//! caller's job (the frontend), same as any other provider creation.
//!
//! The broker dedupes by a stable per-install id. Rather than trust a value
//! supplied by the caller (which the renderer/Electron layer would have to
//! separately generate and could omit, replay, or spoof), dream-core mints
//! and persists its own — the same `system_default_user`-scoped client
//! preference store already used for this deployment's other local-only
//! settings (see `PROVIDER_CREDENTIAL_OWNER` in `routes.rs` for why this
//! single-tenant desktop install treats that id as its identity).

use std::sync::Arc;

use dream_core_api_types::TrialKeyResponse;
use dream_core_db::IClientPreferenceRepository;
use serde::Deserialize;

use crate::error::SystemError;

/// The account this deployment's own local-only settings live under — same
/// constant value as `PROVIDER_CREDENTIAL_OWNER` in `routes.rs` (not shared
/// directly to avoid a cross-module coupling for one string; both are pinned
/// to the same "single desktop install" identity).
const LOCAL_INSTALL_OWNER: &str = "system_default_user";

const INSTALL_ID_PREF_KEY: &str = "trial_broker_install_id";

#[derive(Deserialize)]
struct BrokerErrorBody {
    #[serde(default)]
    error: String,
}

/// Requests a trial key from the configured broker. `None` broker URL means
/// this deployment never wired one up — reported plainly rather than
/// silently doing nothing, same convention as `managed_provider_sync`.
#[derive(Clone)]
pub struct TrialKeyService {
    broker_base_url: Option<String>,
    http_client: reqwest::Client,
    client_pref_repo: Arc<dyn IClientPreferenceRepository>,
}

impl TrialKeyService {
    pub fn new(
        broker_base_url: Option<String>,
        http_client: reqwest::Client,
        client_pref_repo: Arc<dyn IClientPreferenceRepository>,
    ) -> Self {
        Self {
            broker_base_url,
            http_client,
            client_pref_repo,
        }
    }

    pub async fn request_trial_key(&self) -> Result<TrialKeyResponse, SystemError> {
        let Some(base_url) = self.broker_base_url.as_deref() else {
            return Err(SystemError::BadRequest(
                "trial key issuance is not configured on this deployment".into(),
            ));
        };

        let install_id = self.get_or_create_install_id().await?;

        let url = format!("{}/v1/trial-keys", base_url.trim_end_matches('/'));
        let response = self
            .http_client
            .post(&url)
            .json(&serde_json::json!({ "install_id": install_id }))
            .send()
            .await
            .map_err(|e| SystemError::BadGateway(format!("could not reach trial key broker: {e}")))?;

        let status = response.status();
        if status.is_success() {
            return response
                .json::<TrialKeyResponse>()
                .await
                .map_err(|e| SystemError::BadGateway(format!("trial key broker returned an unexpected response: {e}")));
        }

        let reason = response
            .json::<BrokerErrorBody>()
            .await
            .map(|b| b.error)
            .unwrap_or_default();

        Err(match status.as_u16() {
            409 => SystemError::Conflict("this device has already claimed a trial model key".into()),
            429 => SystemError::RateLimited,
            503 => {
                SystemError::ServiceUnavailable("today's trial key budget has been used up, please try again tomorrow".into())
            }
            _ => SystemError::BadGateway(format!("trial key broker rejected the request ({status}): {reason}")),
        })
    }

    /// This deployment's stable id for the broker's per-device dedup check.
    /// Generated once and persisted alongside this install's other local
    /// settings — never regenerated, so a device that already claimed a
    /// trial key keeps getting the same 409 on every retry rather than
    /// silently minting a fresh identity to route around the broker's limit.
    async fn get_or_create_install_id(&self) -> Result<String, SystemError> {
        let existing = self
            .client_pref_repo
            .get_by_keys(LOCAL_INSTALL_OWNER, &[INSTALL_ID_PREF_KEY])
            .await
            .map_err(|e| SystemError::Internal(format!("failed to read install id: {e}")))?;

        if let Some(row) = existing.into_iter().next() {
            let id: String = serde_json::from_str(&row.value).unwrap_or(row.value);
            if !id.trim().is_empty() {
                return Ok(id);
            }
        }

        let id = dream_core_common::generate_prefixed_id("install");
        let serialized = serde_json::to_string(&id)
            .map_err(|e| SystemError::Internal(format!("failed to serialize install id: {e}")))?;
        self.client_pref_repo
            .upsert_batch(LOCAL_INSTALL_OWNER, &[(INSTALL_ID_PREF_KEY, serialized.as_str())])
            .await
            .map_err(|e| SystemError::Internal(format!("failed to persist install id: {e}")))?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_db::SqliteClientPreferenceRepository;
    use dream_core_db::init_database_memory;

    async fn service_with_broker(broker_base_url: Option<String>) -> TrialKeyService {
        let db = init_database_memory().await.unwrap();
        let repo: Arc<dyn IClientPreferenceRepository> = Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()));
        TrialKeyService::new(broker_base_url, reqwest::Client::new(), repo)
    }

    #[tokio::test]
    async fn no_broker_configured_reports_plainly() {
        let service = service_with_broker(None).await;
        let err = service.request_trial_key().await.unwrap_err();
        assert!(matches!(err, SystemError::BadRequest(_)));
    }

    #[tokio::test]
    async fn install_id_is_generated_once_and_then_stable() {
        let service = service_with_broker(Some("http://127.0.0.1:1".to_owned())).await;
        let first = service.get_or_create_install_id().await.unwrap();
        let second = service.get_or_create_install_id().await.unwrap();
        assert_eq!(first, second, "install id must not change across calls");
        assert!(!first.trim().is_empty());
    }
}
