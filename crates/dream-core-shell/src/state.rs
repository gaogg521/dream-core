use std::sync::Arc;

use dream_core_system::{ClientPrefService, ProviderService};

use crate::shell::ShellService;
use crate::stt::SttService;

#[derive(Clone)]
pub struct ShellRouterState {
    pub shell_service: Arc<ShellService>,
    pub stt_service: Arc<SttService>,
    pub client_pref_service: ClientPrefService,
    /// Optional only for isolated shell-route tests. Production wiring always
    /// supplies this so an STT selection can reuse a configured model channel.
    pub provider_service: Option<ProviderService>,
}
