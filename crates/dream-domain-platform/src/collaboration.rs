//! Realtime collaboration seam (P2-2).
//!
//! Default adapter HTTP-probes the saved relay URL (`wss://` is rewritten to HTTPS).

use async_trait::async_trait;

use crate::http_probe::probe_http_endpoint;

/// Outcome of a collaboration-backend probe.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollaborationStatus {
    /// `"not_configured"`, `"ok"`, or `"error"`.
    pub status: String,
    pub message: String,
}

/// Non-secret + secret collaboration config handed to a provider for a probe.
/// `secret` is already decrypted; never log it.
pub struct CollaborationSettings<'a> {
    pub provider: Option<&'a str>,
    pub endpoint: Option<&'a str>,
    pub secret: Option<&'a str>,
    pub presence: bool,
}

#[async_trait]
pub trait CollaborationProvider: Send + Sync {
    /// Probe whether the configured collaboration backend can be reached.
    async fn probe(&self, settings: CollaborationSettings<'_>) -> CollaborationStatus;
}

/// Default provider: HTTP GET the relay (Bearer token if stored).
pub struct NoopCollaborationProvider;

#[async_trait]
impl CollaborationProvider for NoopCollaborationProvider {
    async fn probe(&self, settings: CollaborationSettings<'_>) -> CollaborationStatus {
        let (status, message) = probe_http_endpoint(settings.endpoint, settings.secret).await;
        CollaborationStatus { status, message }
    }
}
