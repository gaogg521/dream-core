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

use dream_core_api_types::{TrialKeyResponse, TrialQuotaStatusResponse};
use dream_core_db::IClientPreferenceRepository;
use serde::Deserialize;

use crate::error::SystemError;

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
            return response.json::<TrialKeyResponse>().await.map_err(|e| {
                SystemError::BadGateway(format!("trial key broker returned an unexpected response: {e}"))
            });
        }

        let reason = response
            .json::<BrokerErrorBody>()
            .await
            .map(|b| b.error)
            .unwrap_or_default();

        Err(match status.as_u16() {
            409 => SystemError::Conflict("this device has already claimed a trial model key".into()),
            429 => SystemError::RateLimited,
            503 => SystemError::ServiceUnavailable(
                "today's trial key budget has been used up, please try again tomorrow".into(),
            ),
            _ => SystemError::BadGateway(format!("trial key broker rejected the request ({status}): {reason}")),
        })
    }

    /// Asks the broker where this install's trial allowance stands.
    ///
    /// The desktop talks to the model provider directly, so without this the
    /// only way it learns the allowance is spent is by being refused
    /// mid-request. The broker reads the position from the upstream by the
    /// key's handle — neither it nor this service ever holds the key itself.
    pub async fn read_quota_status(&self) -> Result<TrialQuotaStatusResponse, SystemError> {
        let Some(base_url) = self.broker_base_url.as_deref() else {
            return Err(SystemError::BadRequest(
                "trial key issuance is not configured on this deployment".into(),
            ));
        };

        let install_id = self.get_or_create_install_id().await?;

        let url = format!("{}/v1/quota/status", base_url.trim_end_matches('/'));
        let response = self
            .http_client
            .post(&url)
            .json(&serde_json::json!({ "install_id": install_id }))
            .send()
            .await
            .map_err(|e| SystemError::BadGateway(format!("could not reach trial key broker: {e}")))?;

        let status = response.status();
        if status.is_success() {
            return response.json::<TrialQuotaStatusResponse>().await.map_err(|e| {
                SystemError::BadGateway(format!("trial key broker returned an unexpected response: {e}"))
            });
        }

        let reason = response
            .json::<BrokerErrorBody>()
            .await
            .map(|b| b.error)
            .unwrap_or_default();

        Err(match status.as_u16() {
            // Never claimed a key here. Not a failure — the honest answer, and
            // the caller uses it to know there is nothing to show.
            404 => SystemError::NotFound("this device has not claimed a trial model key".into()),
            429 => SystemError::RateLimited,
            _ => SystemError::BadGateway(format!("trial key broker rejected the request ({status}): {reason}")),
        })
    }

    /// This deployment's stable id for the broker's per-device dedup check.
    /// Shared with [`crate::metered_access`] so mode A and mode B present the
    /// same identity for one device — see [`crate::install_id`].
    async fn get_or_create_install_id(&self) -> Result<String, SystemError> {
        crate::install_id::get_or_create_install_id(&self.client_pref_repo).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_db::SqliteClientPreferenceRepository;
    use dream_core_db::init_database_memory;

    async fn service_with_broker(broker_base_url: Option<String>) -> TrialKeyService {
        let db = init_database_memory().await.unwrap();
        let repo: Arc<dyn IClientPreferenceRepository> =
            Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()));
        TrialKeyService::new(broker_base_url, reqwest::Client::new(), repo)
    }

    #[tokio::test]
    async fn no_broker_configured_reports_plainly() {
        let service = service_with_broker(None).await;
        let err = service.request_trial_key().await.unwrap_err();
        assert!(matches!(err, SystemError::BadRequest(_)));
    }

    /// Same for the quota read: an unconfigured deployment says so rather than
    /// reporting an empty allowance, which would read as "you have nothing
    /// left" instead of "this was never set up".
    #[tokio::test]
    async fn quota_without_a_broker_reports_plainly_too() {
        let service = service_with_broker(None).await;
        let err = service.read_quota_status().await.unwrap_err();
        assert!(matches!(err, SystemError::BadRequest(_)));
    }

    /// Both calls resolve the same install id. If they diverged, the quota
    /// read would ask about a key belonging to a different identity and answer
    /// 404 for a device that does hold one.
    #[tokio::test]
    async fn the_quota_read_uses_the_same_install_id_as_issuance() {
        let service = service_with_broker(Some("http://127.0.0.1:1".to_owned())).await;
        let first = service.get_or_create_install_id().await.unwrap();

        // Both paths are unreachable at this address; what matters is that
        // neither minted a second identity on the way.
        let _ = service.request_trial_key().await;
        let _ = service.read_quota_status().await;

        assert_eq!(service.get_or_create_install_id().await.unwrap(), first);
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
