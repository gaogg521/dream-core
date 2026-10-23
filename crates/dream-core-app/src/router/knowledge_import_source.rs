//! Lets knowledge imports read the company's registered file storages.
//!
//! `dream-domain-devops` owns the import and must not depend on the platform
//! plane that owns the storages, so this adapter is the seam: it resolves the
//! administrator's company, then forwards to the platform's object-storage
//! service, which already confines every listing and read to the storage's own
//! prefix.

use std::sync::Arc;

use async_trait::async_trait;
use dream_domain_devops::{ExternalEntry, ExternalListing, ExternalObjectSource};
use dream_domain_platform::PlatformService;

pub(crate) struct PlatformObjectSource {
    platform: Arc<PlatformService>,
}

impl PlatformObjectSource {
    pub(crate) fn new(platform: Arc<PlatformService>) -> Self {
        Self { platform }
    }

    /// The company the administrator works in. Without one there is nothing to read.
    async fn tenant_of(&self, actor: &str) -> Result<String, String> {
        match self.platform.resolve_actor(actor).await {
            Ok(Some(actor)) => Ok(actor.tenant_id),
            Ok(None) => Err("the administrator does not belong to a company".to_owned()),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// Mirrors the platform's own prefix shape: no leading slash, one trailing
/// slash when non-empty.
fn normalize(sub_prefix: &str) -> String {
    let trimmed = sub_prefix.trim().trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

#[async_trait]
impl ExternalObjectSource for PlatformObjectSource {
    async fn storage_exists(&self, actor: &str, storage_id: &str) -> Result<bool, String> {
        let tenant = self.tenant_of(actor).await?;
        let configs = self
            .platform
            .list_object_storage_configs(&tenant)
            .await
            .map_err(|e| e.to_string())?;
        Ok(configs.iter().any(|config| config.id == storage_id))
    }

    async fn list(
        &self,
        actor: &str,
        storage_id: &str,
        sub_prefix: &str,
        token: Option<&str>,
    ) -> Result<ExternalListing, String> {
        let tenant = self.tenant_of(actor).await?;
        let sub = normalize(sub_prefix);
        let listing = self
            .platform
            .list_object_storage_entries(&tenant, storage_id, Some(&sub), token, 1000)
            .await
            .map_err(|e| e.to_string())?;
        // The listing reports the full prefix (`<configured prefix><sub>`); what
        // the import needs to recurse is the part relative to the configured one.
        let base = listing.prefix.strip_suffix(&sub).unwrap_or(&listing.prefix).to_owned();
        Ok(ExternalListing {
            entries: listing
                .entries
                .into_iter()
                .map(|entry| ExternalEntry {
                    rel: entry.key.strip_prefix(&base).unwrap_or(&entry.key).to_owned(),
                    key: entry.key,
                    is_prefix: entry.is_prefix,
                    size_bytes: entry.size_bytes,
                    last_modified: entry.last_modified,
                    etag: entry.etag,
                })
                .collect(),
            next_token: listing.next_token,
        })
    }

    async fn read(&self, actor: &str, storage_id: &str, key: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        let tenant = self.tenant_of(actor).await?;
        self.platform
            .read_object_storage_object(&tenant, storage_id, key, max_bytes)
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn prefixes_take_the_platforms_shape() {
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("/a//"), "a/");
        assert_eq!(normalize("a/b"), "a/b/");
    }
}
