//! Runtime-only company model providers. Credentials remain in the proxy;
//! no per-member proxy token is written into the deployment-global provider table.
use dream_core_db::models::Provider;
use dream_core_db::{CreateProviderParams, DbError, IProviderRepository, UpdateProviderParams};
use std::sync::{Arc, OnceLock};

pub(crate) struct EnterpriseProviderRepository {
    inner: Arc<dyn IProviderRepository>,
    devops: OnceLock<Arc<dream_domain_devops::DevopsService>>,
    key: [u8; 32],
    base_url: String,
}

impl EnterpriseProviderRepository {
    pub(crate) fn new(inner: Arc<dyn IProviderRepository>, key: [u8; 32], base_url: String) -> Self {
        Self {
            inner,
            devops: OnceLock::new(),
            key,
            base_url,
        }
    }

    pub(crate) fn bind(&self, service: Arc<dream_domain_devops::DevopsService>) {
        // Admin and app routers can share one AppServices instance.
        let _ = self.devops.set(service);
    }
}

#[async_trait::async_trait]
impl IProviderRepository for EnterpriseProviderRepository {
    async fn list(&self, user_id: &str) -> Result<Vec<Provider>, DbError> {
        self.inner.list(user_id).await
    }

    async fn find_by_id(&self, user_id: &str, id: &str) -> Result<Option<Provider>, DbError> {
        let Some(channel_id) = id.strip_prefix("prov_chan_") else {
            return self.inner.find_by_id(user_id, id).await;
        };
        let Some(service) = self.devops.get() else {
            return Ok(None);
        };
        let channel = service
            .list_provider_channels(user_id)
            .await
            .map_err(|e| DbError::Init(e.to_string()))?
            .into_iter()
            .find(|c| c.id == channel_id && c.enabled);
        let Some(channel) = channel else {
            return Ok(None);
        };
        let token = service
            .issue_runtime_channel_token(user_id, channel_id)
            .await
            .map_err(|e| DbError::Init(e.to_string()))?;
        let encrypted =
            dream_core_common::encrypt_string(&token, &self.key).map_err(|e| DbError::Crypto(e.to_string()))?;
        let now = dream_core_common::now_ms();
        Ok(Some(Provider {
            id: id.into(),
            user_id: user_id.into(),
            platform: channel.platform,
            name: channel.name,
            base_url: format!(
                "{}/api/one/model-proxy/{}",
                self.base_url.trim_end_matches('/'),
                channel_id
            ),
            api_key_encrypted: encrypted,
            models: channel.models,
            enabled: true,
            capabilities: "[]".into(),
            context_limit: None,
            model_protocols: channel.model_protocols,
            model_enabled: None,
            model_health: None,
            model_settings: channel.model_settings.unwrap_or_else(|| "{}".into()),
            bedrock_config: None,
            is_full_url: false,
            managed_by: Some("enterprise".into()),
            created_at: now,
            updated_at: now,
        }))
    }

    async fn create(&self, params: CreateProviderParams<'_>) -> Result<Provider, DbError> {
        self.inner.create(params).await
    }
    async fn update(&self, user_id: &str, id: &str, params: UpdateProviderParams<'_>) -> Result<Provider, DbError> {
        if id.starts_with("prov_chan_") {
            return Err(DbError::Conflict(
                "Company providers are managed in model channels".into(),
            ));
        }
        self.inner.update(user_id, id, params).await
    }
    async fn delete(&self, user_id: &str, id: &str) -> Result<(), DbError> {
        if id.starts_with("prov_chan_") {
            return Err(DbError::Conflict(
                "Company providers are managed in model channels".into(),
            ));
        }
        self.inner.delete(user_id, id).await
    }
}
