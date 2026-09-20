//! Client-preference gate for cross-session delivery.

use async_trait::async_trait;
use dream_core_conversation::session_delivery::{SessionDeliveryGate, CLIENT_PREF_CROSS_SESSION};
use dream_core_system::ClientPrefService;

#[derive(Clone)]
pub struct ClientPrefSessionDeliveryGate {
    pub client_pref: ClientPrefService,
}

#[async_trait]
impl SessionDeliveryGate for ClientPrefSessionDeliveryGate {
    async fn delivery_enabled(&self, user_id: &str) -> bool {
        let Ok(prefs) = self
            .client_pref
            .get_preferences(user_id, Some(&[CLIENT_PREF_CROSS_SESSION]))
            .await
        else {
            return true;
        };
        prefs
            .get(CLIENT_PREF_CROSS_SESSION)
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }
}
