//! S3 driver — every public cloud (Alibaba OSS, Tencent COS, Huawei OBS,
//! Qiniu, AWS) and every self-hosted gateway (MinIO, Ceph) speaks this.
//!
//! Lifted out of `object_storage.rs` unchanged when the driver seam landed;
//! the only new thing here is that it implements `StorageDriver`.

use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::config::{BehaviorVersion, Region};
use aws_sdk_s3::{Client as S3Client, Config as S3Config};

use crate::error::PlatformError;
use crate::storage_driver::{DriverConfig, DriverEntry, DriverListing, StorageDriver};

pub struct S3Driver;

/// Collapse an SDK error to the vendor's own code.
///
/// `DisplayErrorContext` renders the whole smithy chain — hundreds of
/// characters of `ServiceError(Unhandled(...))` that end up in a toast. The
/// code ("NoSuchBucket", "AccessDenied") is the part an operator acts on.
fn s3_message<E, R>(err: &aws_sdk_s3::error::SdkError<E, R>) -> String
where
    E: std::error::Error + 'static,
    R: std::fmt::Debug,
{
    let full = format!("{}", aws_sdk_s3::error::DisplayErrorContext(err));
    if let Some(start) = full.find('(')
        && let Some(end) = full[start..].find(')')
    {
        let code = &full[start + 1..start + end];
        if !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric()) {
            return code.to_owned();
        }
    }
    full.lines().next().unwrap_or("storage request failed").to_owned()
}

fn client(cfg: &DriverConfig) -> S3Client {
    let conf = S3Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(cfg.region.clone()))
        .endpoint_url(cfg.endpoint.clone())
        // MinIO and most self-hosted gateways serve path-style only; the
        // public clouds want virtual-host style.
        .force_path_style(cfg.force_path_style)
        .credentials_provider(Credentials::new(
            cfg.access_key_id.clone(),
            cfg.secret_access_key.clone(),
            None,
            None,
            "one-work-console",
        ))
        .build();
    S3Client::from_conf(conf)
}

#[async_trait]
impl StorageDriver for S3Driver {
    async fn probe(&self, cfg: &DriverConfig) -> Result<String, PlatformError> {
        client(cfg)
            .head_bucket()
            .bucket(&cfg.bucket)
            .send()
            .await
            .map(|_| format!("bucket {} reachable", cfg.bucket))
            .map_err(|e| PlatformError::BadRequest(s3_message(&e)))
    }

    async fn list(
        &self,
        cfg: &DriverConfig,
        prefix: &str,
        token: Option<&str>,
        limit: i32,
    ) -> Result<DriverListing, PlatformError> {
        let mut req = client(cfg)
            .list_objects_v2()
            .bucket(&cfg.bucket)
            .delimiter("/")
            .max_keys(limit.clamp(1, 1000));
        if !prefix.is_empty() {
            req = req.prefix(prefix);
        }
        if let Some(token) = token.filter(|t| !t.is_empty()) {
            req = req.continuation_token(token);
        }

        let out = req
            .send()
            .await
            .map_err(|e| PlatformError::BadRequest(s3_message(&e)))?;

        let mut entries: Vec<DriverEntry> = out
            .common_prefixes()
            .iter()
            .filter_map(|cp| cp.prefix())
            .map(|p| DriverEntry {
                name: p.trim_start_matches(prefix).trim_end_matches('/').to_owned(),
                key: p.to_owned(),
                is_prefix: true,
                size_bytes: None,
                last_modified: None,
            })
            .collect();

        entries.extend(
            out.contents()
                .iter()
                .filter(|o| o.key().is_some_and(|k| k != prefix))
                .map(|o| {
                    let key = o.key().unwrap_or_default().to_owned();
                    DriverEntry {
                        name: key.trim_start_matches(prefix).to_owned(),
                        key,
                        is_prefix: false,
                        size_bytes: o.size(),
                        last_modified: o.last_modified().map(|t| t.to_millis().unwrap_or(0)),
                    }
                }),
        );

        Ok(DriverListing {
            entries,
            next_token: out.next_continuation_token().map(str::to_owned),
        })
    }
}
