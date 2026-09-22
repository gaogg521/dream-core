//! Mode A's real-money top-up: relaying create/poll requests to the broker's
//! `/v1/topup/orders` endpoints.
//!
//! Distinct from [`crate::metered_access`]'s package-based orders: this is a
//! scan-to-pay QR for an arbitrary amount, and only exists for vendors whose
//! `TokenVendor` implementation supports it (Baoyun, so far — the broker
//! answers `topup_unsupported` for any other vendor id). dream-core never
//! sees the vendor's system access token; it forwards this install's
//! create/poll requests to the broker and relays the answer, same as every
//! other trial-broker relay in this crate.

use std::sync::Arc;

use dream_core_api_types::TopupOrderResponse;
use dream_core_db::IClientPreferenceRepository;
use serde::Deserialize;

use crate::error::SystemError;
use crate::install_id::get_or_create_install_id;

#[derive(Deserialize)]
struct BrokerErrorBody {
    #[serde(default)]
    error: String,
}

/// Relays this install's top-up requests to the configured broker. `None`
/// broker URL means this deployment never wired one up — reported plainly,
/// same convention as [`crate::trial_key::TrialKeyService`] and
/// [`crate::metered_access::MeteredAccessService`].
#[derive(Clone)]
pub struct TopupService {
    broker_base_url: Option<String>,
    http_client: reqwest::Client,
    client_pref_repo: Arc<dyn IClientPreferenceRepository>,
}

impl TopupService {
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
            .ok_or_else(|| SystemError::BadRequest("top-up is not configured on this deployment".into()))
    }

    async fn install_id(&self) -> Result<String, SystemError> {
        get_or_create_install_id(&self.client_pref_repo).await
    }

    /// Creates a real-money top-up order for `vendor` and returns the
    /// broker's scan-to-pay QR.
    pub async fn create_order(&self, vendor: &str, amount: f64) -> Result<TopupOrderResponse, SystemError> {
        let base_url = self.base_url()?;
        let install_id = self.install_id().await?;
        let url = format!("{base_url}/v1/topup/orders");

        let response = self
            .http_client
            .post(&url)
            .json(&serde_json::json!({
                "vendor": vendor,
                "install_id": install_id,
                "amount": amount,
            }))
            .send()
            .await
            .map_err(reach_error)?;

        parse_broker_json(response).await
    }

    /// Polls one order's status. Same install id as `create_order` — the
    /// broker checks it against the order's echoed `reference` and refuses a
    /// mismatch, so this can't be used to peek at (or credit from) an order
    /// this install didn't create.
    pub async fn get_order(&self, vendor: &str, order_id: &str) -> Result<TopupOrderResponse, SystemError> {
        let base_url = self.base_url()?;
        let install_id = self.install_id().await?;
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("vendor", vendor)
            .append_pair("install_id", &install_id)
            .finish();
        let url = format!("{base_url}/v1/topup/orders/{order_id}?{query}");

        let response = self.http_client.get(&url).send().await.map_err(reach_error)?;

        parse_broker_json(response).await
    }
}

fn reach_error(e: reqwest::Error) -> SystemError {
    SystemError::BadGateway(format!("could not reach the trial broker: {e}"))
}

/// Reads a broker response into `T`, mapping its status codes to
/// `SystemError`. Mirrors `metered_access::parse_broker_json`.
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
        404 => SystemError::NotFound(match reason.as_str() {
            "vendor_unknown" => "no trial vendor by that id on this broker".into(),
            "not_issued" => "this device has not claimed a trial model key on that vendor".into(),
            "topup_order_mismatch" => "no such top-up order".into(),
            _ => "not found".into(),
        }),
        400 => SystemError::BadRequest(match reason.as_str() {
            "topup_unsupported" => "this vendor does not support top-up orders".into(),
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

    async fn service(broker_base_url: Option<String>) -> TopupService {
        let db = init_database_memory().await.unwrap();
        let repo: Arc<dyn IClientPreferenceRepository> =
            Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()));
        TopupService::new(broker_base_url, reqwest::Client::new(), repo)
    }

    #[tokio::test]
    async fn no_broker_configured_reports_plainly() {
        let svc = service(None).await;
        assert!(matches!(
            svc.create_order("baoyun", 10.0).await.unwrap_err(),
            SystemError::BadRequest(_)
        ));
        assert!(matches!(
            svc.get_order("baoyun", "order-1").await.unwrap_err(),
            SystemError::BadRequest(_)
        ));
    }
}
