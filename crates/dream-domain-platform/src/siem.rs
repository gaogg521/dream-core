//! SIEM audit-log export seam (P1-4).
//!
//! HTTP kinds GET the endpoint; `syslog` does a TCP connect.

use async_trait::async_trait;

use crate::http_probe::{probe_http_endpoint, probe_tcp_host};

/// Outcome of a SIEM export/probe.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiemStatus {
    /// `"not_configured"`, `"ok"`, or `"error"`.
    pub status: String,
    pub message: String,
}

/// Non-secret + secret SIEM config handed to an exporter for a probe.
/// `secret` is already decrypted; never log it.
pub struct SiemSettings<'a> {
    pub kind: Option<&'a str>,
    pub endpoint: Option<&'a str>,
    pub secret: Option<&'a str>,
}

#[async_trait]
pub trait SiemExporter: Send + Sync {
    /// Probe whether the configured SIEM endpoint can be reached.
    async fn probe(&self, settings: SiemSettings<'_>) -> SiemStatus;
}

/// Default exporter: HTTP GET, or TCP for syslog.
pub struct NoopSiemExporter;

#[async_trait]
impl SiemExporter for NoopSiemExporter {
    async fn probe(&self, settings: SiemSettings<'_>) -> SiemStatus {
        if settings.kind.map(|k| k.eq_ignore_ascii_case("syslog")).unwrap_or(false) {
            let (status, message) = probe_tcp_host(settings.endpoint, 514);
            return SiemStatus { status, message };
        }
        let (status, message) = probe_http_endpoint(settings.endpoint, settings.secret).await;
        SiemStatus { status, message }
    }
}
