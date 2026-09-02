//! This deployment's stable per-install id for the trial broker's per-device
//! dedup check.
//!
//! Both broker-backed features — mode A ([`crate::trial_key`]) and mode B
//! ([`crate::metered_access`]) — must present the *same* id: a device is one
//! device to the broker whichever way it claimed. So the resolution lives
//! here, not duplicated per service.
//!
//! Rather than trust a value supplied by the caller (which the
//! renderer/Electron layer would have to separately generate and could omit,
//! replay, or spoof), dream-core mints and persists its own — in the same
//! `system_default_user`-scoped client preference store this single-tenant
//! desktop install already uses for its other local-only settings (see
//! `PROVIDER_CREDENTIAL_OWNER` in `routes.rs` for why that id is treated as
//! this install's identity).

use std::sync::Arc;

use dream_core_db::IClientPreferenceRepository;

use crate::error::SystemError;

/// The account this deployment's own local-only settings live under — same
/// constant value as `PROVIDER_CREDENTIAL_OWNER` in `routes.rs` (not shared
/// directly to avoid a cross-module coupling for one string; both are pinned
/// to the same "single desktop install" identity).
const LOCAL_INSTALL_OWNER: &str = "system_default_user";

const INSTALL_ID_PREF_KEY: &str = "trial_broker_install_id";

/// Returns this deployment's install id, generating and persisting one on
/// first call. Never regenerated — a device that already claimed an allowance
/// keeps getting the same broker answer on every retry rather than silently
/// minting a fresh identity to route around the broker's limit.
pub(crate) async fn get_or_create_install_id(
    client_pref_repo: &Arc<dyn IClientPreferenceRepository>,
) -> Result<String, SystemError> {
    let existing = client_pref_repo
        .get_by_keys(LOCAL_INSTALL_OWNER, &[INSTALL_ID_PREF_KEY])
        .await
        .map_err(|e| SystemError::Internal(format!("failed to read install id: {e}")))?;

    if let Some(row) = existing.into_iter().next() {
        let id: String = serde_json::from_str(&row.value).unwrap_or(row.value);
        if !id.trim().is_empty() {
            return Ok(id);
        }
    }

    let id = dream_core_common::generate_prefixed_id("install");
    let serialized = serde_json::to_string(&id)
        .map_err(|e| SystemError::Internal(format!("failed to serialize install id: {e}")))?;
    client_pref_repo
        .upsert_batch(LOCAL_INSTALL_OWNER, &[(INSTALL_ID_PREF_KEY, serialized.as_str())])
        .await
        .map_err(|e| SystemError::Internal(format!("failed to persist install id: {e}")))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dream_core_db::SqliteClientPreferenceRepository;
    use dream_core_db::init_database_memory;

    async fn repo() -> Arc<dyn IClientPreferenceRepository> {
        let db = init_database_memory().await.unwrap();
        Arc::new(SqliteClientPreferenceRepository::new(db.pool().clone()))
    }

    #[tokio::test]
    async fn generated_once_then_stable() {
        let repo = repo().await;
        let first = get_or_create_install_id(&repo).await.unwrap();
        let second = get_or_create_install_id(&repo).await.unwrap();
        assert_eq!(first, second, "install id must not change across calls");
        assert!(!first.trim().is_empty());
    }
}
