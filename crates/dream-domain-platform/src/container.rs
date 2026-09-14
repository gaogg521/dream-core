//! Container runtime seam (P1-3).
//!
//! Default adapter HTTP-probes the saved Docker/K8s API endpoint. A custom
//! `ContainerRuntime` can still be dropped in via `PlatformService::with_container_runtime`.

use async_trait::async_trait;

use crate::http_probe::probe_http_endpoint;

/// Outcome of a container-runtime probe, shaped like `dream_domain_org`'s
/// `IntegrationTestResult` for the same "not configured yet" UX.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerStatus {
    /// `"not_configured"`, `"ok"`, or `"error"`.
    pub status: String,
    pub message: String,
}

/// Non-secret + secret container config handed to a runtime for a live probe.
/// `registry_secret` is already decrypted; never log it.
pub struct ContainerSettings<'a> {
    pub runtime_kind: Option<&'a str>,
    pub endpoint: Option<&'a str>,
    pub default_image: Option<&'a str>,
    pub registry: Option<&'a str>,
    pub registry_secret: Option<&'a str>,
}

#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// Probe whether the configured runtime can be reached / is usable.
    async fn probe(&self, settings: ContainerSettings<'_>) -> ContainerStatus;
}

/// Default runtime: GET the configured HTTP endpoint (Bearer registry secret if set).
pub struct NoopContainerRuntime;

#[async_trait]
impl ContainerRuntime for NoopContainerRuntime {
    async fn probe(&self, settings: ContainerSettings<'_>) -> ContainerStatus {
        let (status, message) = probe_http_endpoint(settings.endpoint, settings.registry_secret).await;
        ContainerStatus { status, message }
    }
}
