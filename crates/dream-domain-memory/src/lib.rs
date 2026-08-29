//! one-memory: the enterprise memory subsystem (P2-2, align-openocta).
//!
//! Three collection tiers — global (company-wide knowledge, readable by
//! every tenant member), department (fused per-department memory, bound to
//! `department_id`), personal (one member's distillation and preference
//! learning, bound to `owner_user_id`, owner-only) — plus synchronous
//! refinement jobs (merge SHA-256 duplicates, trim low-value items to the
//! active floor) and read/write grants with a coverage metric.
//!
//! Routes mount under `/api/one/*` on the governance plane, so the same
//! assembly both binaries serve. The crate is enterprise-gated like the
//! other `dream-domain-*` crates: personal builds have no memory and no
//! route surface.
//!
//! Kept out of `dream-domain-platform` on purpose: memory is a subsystem
//! (own tables, own lifecycle, own route prefix), not platform config.

pub mod error;
pub mod migrate;
pub mod models;
pub mod rbac;
pub mod routes;
pub mod service;
pub mod state;

pub use error::MemoryError;
pub use migrate::run_one_memory_migrations;
pub use models::{GrantCoverageDto, MemoryCollectionDto, MemoryGrantDto, MemoryItemDto, MemoryRefineJobDto};
pub use rbac::{RequireMemoryAdmin, RequireMemoryMember};
pub use routes::one_memory_routes;
pub use service::{
    MEMORY_REFINE_ACTIVE_FLOOR, MEMORY_REFINE_MIN_IMPORTANCE, MEMORY_SCOPES, MemoryActor, MemoryService,
};
pub use state::OneMemoryRouterState;
