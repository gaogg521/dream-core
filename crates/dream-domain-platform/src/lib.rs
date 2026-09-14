//! one-platform: deployment/platform infrastructure config for the 1ONE
//! Dream Core fork — containerized execution (P1-3), realtime collaboration
//! (P2-2), IM pipelines, and console settings.
//!
//! Default adapters HTTP-probe (or TCP-probe) the saved endpoints. A custom
//! implementation can still be dropped in via `PlatformService::with_*`.

pub mod collaboration;
pub mod container;
pub mod error;
pub mod http_probe;
pub mod ip_allowlist;
pub mod migrate;
pub mod models;
pub mod object_storage;
pub mod rbac;
pub mod routes;
pub mod service;
pub mod siem;
pub mod state;
pub mod storage_driver;
pub mod storage_s3;
pub mod storage_webdav;

pub use collaboration::{CollaborationProvider, CollaborationSettings, CollaborationStatus, NoopCollaborationProvider};
pub use container::{ContainerRuntime, ContainerSettings, ContainerStatus, NoopContainerRuntime};
pub use error::PlatformError;
pub use migrate::run_one_platform_migrations;
pub use rbac::RequirePlatformAdmin;
pub use routes::one_platform_routes;
pub use service::{
    ApiKeyAuthOutcome, ConfigImportRow, GRANT_ALL_RESOURCES, GRANT_RESOURCE_TYPES, GRANT_SUBJECT_TYPES, PlatformActor,
    PlatformService,
};
pub use siem::{NoopSiemExporter, SiemExporter, SiemSettings, SiemStatus};
pub use state::OnePlatformRouterState;
