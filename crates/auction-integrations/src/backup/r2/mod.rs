//! 只消费显式 TOML 凭据的 R2 适配器；不提供覆盖、对象删除或环境凭据入口。

use std::time::Duration;

use object_store::{
    ClientOptions, RetryConfig,
    aws::{AmazonS3, AmazonS3Builder, S3ConditionalPut},
};

mod model;
mod upload;
mod verify;

pub use model::{BackupObject, BackupObjectStatus, BackupStoreError};

/// 备份专用 S3 客户端；单次请求和重试均有界，调用方另施加整次运行预算。
pub struct R2BackupStore {
    store: AmazonS3,
}

impl R2BackupStore {
    /// 必须传入完整目的地与成对凭据；不解析环境、profile 或实例元数据。
    pub fn new(
        endpoint: &str,
        bucket: &str,
        access_key: &str,
        secret: &str,
    ) -> Result<Self, BackupStoreError> {
        if access_key.is_empty() || secret.is_empty() {
            return Err(BackupStoreError::Configuration);
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let options = ClientOptions::new()
            .with_allow_http(false)
            .with_connect_timeout(Duration::from_secs(10))
            .with_timeout(Duration::from_secs(300));
        let store = AmazonS3Builder::new()
            .with_endpoint(endpoint)
            .with_bucket_name(bucket)
            .with_region("auto")
            .with_access_key_id(access_key)
            .with_secret_access_key(secret)
            .with_virtual_hosted_style_request(false)
            .with_conditional_put(S3ConditionalPut::ETagMatch)
            .with_client_options(options)
            .with_retry(RetryConfig {
                max_retries: 2,
                retry_timeout: Duration::from_secs(30),
                ..Default::default()
            })
            .build()
            .map_err(|_| BackupStoreError::Configuration)?;
        Ok(Self { store })
    }
}
