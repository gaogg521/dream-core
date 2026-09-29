//! Mode D — hosted default speech-to-text, resolved through the company's
//! broker (`dream-trial-broker`).
//!
//! Same shape and reasoning as [`crate::trial_key`]: dream-core never holds a
//! vendor STT key. This module only forwards this install's audio to the
//! broker and relays back whatever it decides (a transcript, or a refusal
//! because the broker has no STT vendor configured / today's spend budget is
//! spent / this install is rate-limited). The broker resolves the actual
//! vendor (Aliyun today) so a vendor swap on the broker side needs no
//! dream-core release.

use std::sync::Arc;

use base64::Engine as _;
use dream_core_db::IClientPreferenceRepository;
use serde::Deserialize;

use crate::error::SystemError;

#[derive(Deserialize)]
struct BrokerErrorBody {
    #[serde(default)]
    error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HostedSttQuota {
    pub used_today: i64,
    pub daily_limit: i64,
    pub remaining: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HostedSttResponse {
    pub provider: String,
    pub text: String,
    pub quota: HostedSttQuota,
}

/// Requests a transcription from the configured broker. `None` broker URL
/// means this deployment never wired one up.
#[derive(Clone)]
pub struct HostedSttService {
    broker_base_url: Option<String>,
    http_client: reqwest::Client,
    client_pref_repo: Arc<dyn IClientPreferenceRepository>,
}

impl HostedSttService {
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

    /// Whether this deployment has a broker configured at all — the settings
    /// UI and `load_stt_config`'s zero-config default both need this to
    /// decide whether "hosted" is a real option, not just a menu item that
    /// will always fail.
    pub fn is_configured(&self) -> bool {
        self.broker_base_url.is_some()
    }

    /// Resolve the broker WebSocket endpoint and this installation's opaque
    /// quota id. The caller uses the id only in the first broker frame; no
    /// vendor credential ever leaves the broker.
    pub async fn stream_endpoint(&self) -> Result<(String, String), SystemError> {
        let Some(base_url) = self.broker_base_url.as_deref() else {
            return Err(SystemError::BadRequest(
                "hosted speech-to-text is not configured on this deployment".into(),
            ));
        };
        let base = base_url.trim_end_matches('/');
        let ws_base = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            return Err(SystemError::BadRequest(
                "hosted speech-to-text broker URL must use http or https".into(),
            ));
        };
        let install_id = crate::install_id::get_or_create_install_id(&self.client_pref_repo).await?;
        Ok((format!("{ws_base}/v1/stt/stream"), install_id))
    }

    /// `audio` is the clip's raw bytes; `mime_type` may still carry codec
    /// parameters (`audio/webm;codecs=opus`) — the broker strips them.
    pub async fn transcribe(
        &self,
        audio: &[u8],
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<HostedSttResponse, SystemError> {
        let Some(base_url) = self.broker_base_url.as_deref() else {
            return Err(SystemError::BadRequest(
                "hosted speech-to-text is not configured on this deployment".into(),
            ));
        };

        let install_id = crate::install_id::get_or_create_install_id(&self.client_pref_repo).await?;
        let audio_base64 = base64::engine::general_purpose::STANDARD.encode(audio);

        let url = format!("{}/v1/stt", base_url.trim_end_matches('/'));
        let response = self
            .http_client
            .post(&url)
            .json(&serde_json::json!({
                "install_id": install_id,
                "audio_base64": audio_base64,
                "mime_type": mime_type,
                "language": language,
            }))
            .send()
            .await
            .map_err(|e| SystemError::BadGateway(format!("could not reach hosted stt broker: {e}")))?;

        let status = response.status();
        if status.is_success() {
            return response.json::<HostedSttResponse>().await.map_err(|e| {
                SystemError::BadGateway(format!("hosted stt broker returned an unexpected response: {e}"))
            });
        }

        let reason = response
            .json::<BrokerErrorBody>()
            .await
            .map(|b| b.error)
            .unwrap_or_default();

        Err(match status.as_u16() {
            429 => SystemError::RateLimited,
            503 if reason == "stt_unavailable" => {
                SystemError::BadRequest("hosted speech-to-text is not configured on this deployment".into())
            }
            503 => SystemError::ServiceUnavailable(
                "today's hosted transcription budget has been used up, please try again tomorrow".into(),
            ),
            _ => SystemError::BadGateway(format!("hosted stt broker rejected the request ({status}): {reason}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_db::SqliteClientPreferenceRepository;
    use dream_core_db::init_database_memory;

    async fn service(broker_base_url: Option<String>) -> HostedSttService {
        let db = init_database_memory().await.unwrap();
        let repo: Arc<dyn IClientPreferenceRepository> =
            Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()));
        HostedSttService::new(broker_base_url, reqwest::Client::new(), repo)
    }

    #[tokio::test]
    async fn unconfigured_deployment_is_reported_plainly() {
        let svc = service(None).await;
        assert!(!svc.is_configured());
        let err = svc.transcribe(&[0u8; 4], "audio/wav", None).await.unwrap_err();
        assert!(matches!(err, SystemError::BadRequest(_)));
    }

    #[tokio::test]
    async fn configured_deployment_reports_configured() {
        let svc = service(Some("http://127.0.0.1:1".into())).await;
        assert!(svc.is_configured());
    }

    #[tokio::test]
    async fn unreachable_broker_is_a_bad_gateway() {
        // Nothing listens on this port; the request must fail as a gateway
        // error, not silently succeed or panic.
        let svc = service(Some("http://127.0.0.1:1".into())).await;
        let err = svc.transcribe(&[0u8; 4], "audio/wav", None).await.unwrap_err();
        assert!(matches!(err, SystemError::BadGateway(_)));
    }
}
