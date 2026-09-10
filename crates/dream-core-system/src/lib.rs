#![warn(clippy::disallowed_types)]

//! System services: provider management, model fetching, settings, and version checks.
/// Platforms that carry no bearer key, and why.
///
/// `bedrock` authenticates through `bedrock_config` (IAM profile or static
/// keys), not an HTTP endpoint plus a bearer token. `ollama` is a daemon on
/// localhost with no authentication at all — there is no key for a user to
/// enter, and the settings dialog correctly does not ask for one.
///
/// This lives here, shared, because the same question is asked in two places
/// that drifted apart: the model-list fetch already exempted both, while
/// provider creation still demanded a key from everyone. The result was a
/// dialog that let you browse an Ollama daemon's models and then refused to
/// save the one you picked, with `apiKey is required` behind a generic
/// "failed to save" toast. Adding a third keyless platform must not require
/// remembering two files.
pub fn platform_authenticates_without_api_key(platform: &str) -> bool {
    matches!(platform.trim().to_ascii_lowercase().as_str(), "bedrock" | "ollama")
}

pub mod bedrock_probe;
pub mod client_pref;
pub mod content_inspection;
pub mod diagnostics;
pub mod enterprise_upstream;
pub mod error;
pub mod install_id;
pub mod keep_awake;
pub mod managed_provider;
pub mod metered_access;
pub mod model_fetcher;
pub mod model_platforms;
pub mod protocol;
pub mod provider;
pub mod routes;
pub mod runtime_prepare;
pub mod send_policy;
pub mod settings;
pub mod sysinfo;
pub mod team_memory;
pub mod tool_security;
pub mod trial_key;
pub mod version;

pub use bedrock_probe::{ConnectionTestRouterState, ConnectionTestService, connection_test_routes};
pub use client_pref::ClientPrefService;
pub use content_inspection::{ContentBlock, ContentInspectionService, PendingFinding};
pub use diagnostics::FeedbackDiagnosticsService;
pub use enterprise_upstream::{EnterpriseUpstream, EnterpriseUpstreamService};
pub use error::SystemError;
pub use keep_awake::{KeepAwakeController, NoopKeepAwakeController, SystemKeepAwakeController};
pub use metered_access::MeteredAccessService;
pub use model_fetcher::ModelFetchService;
pub use protocol::ProtocolDetectionService;
pub use provider::ProviderService;
pub use routes::{SystemRouterState, settings_routes, system_routes};
pub use runtime_prepare::RuntimePrepareService;
pub use send_policy::{SendPolicy, SendPolicyService};
pub use settings::SettingsService;
pub use team_memory::{TeamMemoryItem, TeamMemoryService, TeamMemorySnapshot};
pub use tool_security::{ToolSecurityPolicy, ToolSecurityService};
pub use trial_key::TrialKeyService;
pub use version::VersionCheckService;
