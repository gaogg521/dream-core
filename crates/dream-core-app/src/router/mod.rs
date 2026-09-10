//! HTTP router assembly for the application.

mod antigravity_hook;
mod clipboard_writer;
mod fs_monitor;
mod health;
mod item_revealer;
mod routes;
mod runtime_team_tools;
mod scm_monitor;
mod state;
mod system_file_opener;
mod team_capability_resolver;
mod team_conversation_adapters;
mod trace;

/// Personal-build recall over the memory the renderer syncs down. Deliberately
/// NOT behind `feature = "enterprise"` in the sense that matters — an enterprise
/// member's desktop client runs a PERSONAL build, so this is the only recall
/// their turns ever reach.
///
/// It is nonetheless gated the other way, to `not(enterprise)`, and the two
/// facts are not in tension: under `--features enterprise`,
/// `OneMemoryContextProvider` fills the slot, nothing constructs this type, and
/// unlike its sibling below it is not re-exported from `lib.rs` — so it is
/// reachable from nowhere and `-D warnings` calls it dead. Gating it is what
/// keeps the enterprise edition compiling.
#[cfg(not(feature = "enterprise"))]
pub use routes::LocalTeamMemoryRecall;
/// Personal-build tool-call gate over the policy the renderer syncs down, and
/// there for the same reason as `LocalTeamMemoryRecall`. Ungated because
/// `lib.rs` re-exports it, which keeps it public API — and therefore live — in
/// both editions.
pub use routes::LocalToolSecurityGate;
#[cfg(feature = "enterprise")]
pub use routes::create_admin_router;
/// Lives with the other one-billing/one-platform adapters in `routes.rs`,
/// but is wired in `AppServices` — the agent factory is built before any
/// router exists.
#[cfg(feature = "enterprise")]
pub(crate) use routes::{
    BillingModelAllowlistGate, OneMemoryContextProvider, PlatformToolCallSecurityGate, PolicyGrace,
};
pub use routes::{
    RouterRuntime, create_router, create_router_with_all_state, create_router_with_runtime, create_router_with_states,
};
pub use state::{
    ChannelOrchestratorComponents, ModuleStates, RouterBuildError, build_assistant_state, build_conversation_state,
    build_extension_states, build_module_states, build_ws_state,
};
