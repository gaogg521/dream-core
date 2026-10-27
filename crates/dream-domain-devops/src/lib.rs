#![warn(clippy::disallowed_types)]

//! one-devops: requirements board (issues) + enterprise collaboration
//! registries (skills / MCP / RAG document metadata) for the 1ONE Dream Core
//! fork.
//!
//! Rebuild of the 1ONE ClaudeCode DevOps slice that backs the
//! superAssistant `IssuesWorkbench` and `EnterpriseCollaborationContext`
//! panels. Same own-crate policy as one-org: all state lives in `one_*`
//! tables managed by our own migrator; the only upstream touch point is a
//! route merge in dream-app.
//!
//! RAG: `one_rag_documents` + `one_rag_chunks` + `one_rag_config` back a
//! chunk → embed → store → retrieve pipeline over an OpenAI-compatible
//! embedding endpoint (see `embedding` module).
//!
//! Retrieval is hybrid (see `retrieval`): the dense vector ranking is fused
//! with a BM25 ranking from SQLite's built-in FTS5. The lexical index is a
//! derived structure that can be rebuilt from `one_rag_chunks` at any time,
//! which is what makes the startup backfill and any re-index safe.

pub mod api_assets;
mod automatic_indexing;
pub mod breakdown;
mod catalog_guides;
pub mod config_resolver;
pub mod dlp_service;
pub mod embedding;
pub mod error;
pub mod grants;
mod knowledge;
pub mod knowledge_import;
pub mod market_sync;
pub mod mcp_selection;
mod migrate;
pub mod model_proxy;
pub use market_sync::{MarketFetcher, MarketSourceDto, MarketSyncReportDto, ReqwestFetcher};
pub mod models;
mod office_routes;
pub mod office_service;
pub mod office_session;
mod office_text;
pub mod onnxruntime;
mod pdf_text;
pub mod provider_channel;
pub mod proxy_usage;
mod query_embedding;
mod rag_files;
pub mod resource_pack;
pub mod retrieval;
pub mod routes;
pub mod service;
pub mod state;
mod vector_index;

pub use error::DevopsError;
pub use knowledge_import::{ExternalEntry, ExternalListing, ExternalObjectSource};
pub use migrate::run_one_devops_migrations;
pub use model_proxy::model_proxy_routes;
pub use office_routes::office_engine_routes;
pub use provider_channel::{ExtractionLlmConfig, ResolvedChannel};
pub use proxy_usage::{ProxyUsageEvent, ProxyUsageRecorder};
pub use routes::{admin_devops_routes, one_devops_routes};
pub use service::DevopsService;
pub use state::OneDevopsRouterState;
