//! Mode B: relaying a metered-proxy trial to the company broker.
//!
//! Where [`crate::trial_key`] asks the broker to mint a capped upstream key,
//! this asks it to open a *metered account*: the broker forwards inference
//! under one master key and bills each call against a local ledger. dream-core
//! never holds the master key and never sees model traffic — it forwards this
//! install's claim / quota / order requests to the broker and relays the
//! answer. Wiring the returned `base_url` + `device_token` into a local
//! `providers` row is the frontend's job, same as any other provider.
//!
//! The broker dedupes accounts by the same per-install id mode A uses
//! ([`crate::install_id`]) — one device, one identity, whichever mode.

use std::sync::Arc;

use dream_core_api_types::{MeteredAccessResponse, MeteredOrderResponse, MeteredQuotaStatusResponse};
use dream_core_db::IClientPreferenceRepository;
use serde::Deserialize;

use crate::error::SystemError;
use crate::install_id::get_or_create_install_id;

#[derive(Deserialize)]
struct BrokerErrorBody {
    #[serde(default)]
    error: String,
}

/// Relays this install's metered-proxy requests to the configured broker.
/// `None` broker URL means this deployment never wired one up — reported
/// plainly, same convention as [`crate::trial_key::TrialKeyService`].
#[derive(Clone)]
pub struct MeteredAccessService {
    broker_base_url: Option<String>,
    http_client: reqwest::Client,
    client_pref_repo: Arc<dyn IClientPreferenceRepository>,
}

impl MeteredAccessService {
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

    fn base_url(&self) -> Result<String, SystemError> {
        self.broker_base_url
            .as_deref()
            .map(|u| u.trim_end_matches('/').to_string())
            .ok_or_else(|| SystemError::BadRequest("metered trial access is not configured on this deployment".into()))
    }

    async fn install_id(&self) -> Result<String, SystemError> {
        get_or_create_install_id(&self.client_pref_repo).await
    }

    /// Opens (or re-opens) a metered account for `vendor` and returns what the
    /// client needs to build the provider row.
    pub async fn claim(&self, vendor: &str) -> Result<MeteredAccessResponse, SystemError> {
        let base_url = self.base_url()?;
        let install_id = self.install_id().await?;
        let url = format!("{base_url}/v1/metered/claim");

        let response = self
            .http_client
            .post(&url)
            .json(&serde_json::json!({ "vendor": vendor, "install_id": install_id }))
            .send()
            .await
            .map_err(reach_error)?;

        parse_broker_json(response).await
    }

    /// Where this install's balance stands for `vendor`, from the broker's
    /// local ledger. The desktop otherwise only learns the balance is spent by
    /// being refused mid-request.
    pub async fn read_quota_status(&self, vendor: &str) -> Result<MeteredQuotaStatusResponse, SystemError> {
        let base_url = self.base_url()?;
        let install_id = self.install_id().await?;
        let url = format!("{base_url}/v1/metered/quota/status");

        let response = self
            .http_client
            .post(&url)
            .json(&serde_json::json!({ "vendor": vendor, "install_id": install_id }))
            .send()
            .await
            .map_err(reach_error)?;

        parse_broker_json(response).await
    }

    /// Creates a top-up order and returns the gateway's pay instructions.
    pub async fn create_order(&self, vendor: &str, package_id: &str) -> Result<MeteredOrderResponse, SystemError> {
        let base_url = self.base_url()?;
        let install_id = self.install_id().await?;
        let url = format!("{base_url}/v1/metered/orders");

        let response = self
            .http_client
            .post(&url)
            .json(&serde_json::json!({
                "vendor": vendor,
                "install_id": install_id,
                "package_id": package_id,
            }))
            .send()
            .await
            .map_err(reach_error)?;

        parse_broker_json(response).await
    }

    /// Polls one order's status. Takes no install id — an order id is already
    /// unguessable and scoped to its account.
    pub async fn get_order(&self, order_id: &str) -> Result<MeteredOrderResponse, SystemError> {
        let base_url = self.base_url()?;
        let url = format!("{base_url}/v1/metered/orders/{order_id}");

        let response = self.http_client.get(&url).send().await.map_err(reach_error)?;

        parse_broker_json(response).await
    }
}

fn reach_error(e: reqwest::Error) -> SystemError {
    SystemError::BadGateway(format!("could not reach the trial broker: {e}"))
}

/// Reads a broker response into `T`, mapping its status codes to `SystemError`.
async fn parse_broker_json<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T, SystemError> {
    let status = response.status();
    if status.is_success() {
        return response
            .json::<T>()
            .await
            .map_err(|e| SystemError::BadGateway(format!("trial broker returned an unexpected response: {e}")));
    }

    let reason = response
        .json::<BrokerErrorBody>()
        .await
        .map(|b| b.error)
        .unwrap_or_default();

    Err(match status.as_u16() {
        // Unknown vendor, an install that never claimed, or an order this
        // broker never created — all "there is nothing here", which the
        // caller uses to decide there is nothing to show.
        404 => SystemError::NotFound(match reason.as_str() {
            "metered_vendor_unknown" => "no metered trial vendor by that id".into(),
            "metered_account_unknown" => "this device has not opened a metered trial account".into(),
            "metered_order_unknown" => "no such order".into(),
            _ => "not found".into(),
        }),
        400 => SystemError::BadRequest(match reason.as_str() {
            "metered_package_unknown" => "no top-up package by that id".into(),
            other if !other.is_empty() => other.into(),
            _ => "the trial broker rejected the request".into(),
        }),
        429 => SystemError::RateLimited,
        _ => SystemError::BadGateway(format!("trial broker rejected the request ({status}): {reason}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_db::SqliteClientPreferenceRepository;
    use dream_core_db::init_database_memory;

    async fn service(broker_base_url: Option<String>) -> MeteredAccessService {
        let db = init_database_memory().await.unwrap();
        let repo: Arc<dyn IClientPreferenceRepository> =
            Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()));
        MeteredAccessService::new(broker_base_url, reqwest::Client::new(), repo)
    }

    #[tokio::test]
    async fn no_broker_configured_reports_plainly() {
        let svc = service(None).await;
        assert!(matches!(
            svc.claim("baoyun").await.unwrap_err(),
            SystemError::BadRequest(_)
        ));
        assert!(matches!(
            svc.read_quota_status("baoyun").await.unwrap_err(),
            SystemError::BadRequest(_)
        ));
        assert!(matches!(
            svc.create_order("baoyun", "59").await.unwrap_err(),
            SystemError::BadRequest(_)
        ));
    }

    /// Mode A and mode B must resolve the same install id — the broker dedupes
    /// on it, and a device that claimed one way must look like the same device
    /// the other way.
    #[tokio::test]
    async fn shares_the_install_id_with_mode_a() {
        let db = init_database_memory().await.unwrap();
        let repo: Arc<dyn IClientPreferenceRepository> =
            Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()));

        let metered = MeteredAccessService::new(
            Some("http://127.0.0.1:1".to_owned()),
            reqwest::Client::new(),
            repo.clone(),
        );
        let trial = crate::trial_key::TrialKeyService::new(
            Some("http://127.0.0.1:1".to_owned()),
            reqwest::Client::new(),
            repo.clone(),
        );

        // Both unreachable; what matters is neither minted a second identity.
        let _ = metered.claim("baoyun").await;
        let _ = trial.request_trial_key().await;

        let a = get_or_create_install_id(&repo).await.unwrap();
        let b = metered.install_id().await.unwrap();
        assert_eq!(a, b);
    }
}
