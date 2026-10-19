use std::sync::Arc;

use dream_core_common::{decrypt_string, encrypt_string, now_ms};
use dream_core_db::IChannelRepository;

use crate::error::ChannelError;

/// Private, owner-scoped state used by platform connections across process restarts.
#[async_trait::async_trait]
pub trait PluginStateStore: Send + Sync {
    async fn load(&self, key: &str) -> Result<Option<String>, ChannelError>;
    async fn save(&self, key: &str, value: &str) -> Result<(), ChannelError>;
}

pub(crate) struct EncryptedPluginStateStore {
    repo: Arc<dyn IChannelRepository>,
    owner: String,
    plugin: String,
    encryption_key: [u8; 32],
}

impl EncryptedPluginStateStore {
    pub(crate) fn new(repo: Arc<dyn IChannelRepository>, owner: &str, plugin: &str, encryption_key: [u8; 32]) -> Self {
        Self {
            repo,
            owner: owner.into(),
            plugin: plugin.into(),
            encryption_key,
        }
    }
}

#[async_trait::async_trait]
impl PluginStateStore for EncryptedPluginStateStore {
    async fn load(&self, key: &str) -> Result<Option<String>, ChannelError> {
        self.repo
            .get_plugin_state(&self.owner, &self.plugin, key)
            .await?
            .map(|value| {
                decrypt_string(&value, &self.encryption_key)
                    .map_err(|_| ChannelError::DecryptionFailed("Plugin runtime state could not be decrypted".into()))
            })
            .transpose()
    }

    async fn save(&self, key: &str, value: &str) -> Result<(), ChannelError> {
        let encrypted = encrypt_string(value, &self.encryption_key)
            .map_err(|_| ChannelError::EncryptionFailed("Plugin runtime state could not be encrypted".into()))?;
        self.repo
            .set_plugin_state(&self.owner, &self.plugin, key, &encrypted, now_ms())
            .await?;
        Ok(())
    }
}
