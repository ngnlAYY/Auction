//! HEAD 后顺序分段回读全部字节；每段独立受请求超时约束，完成记录读取有明确上限。

use std::future::poll_fn;

use auction_application::core::lower_hex;
use object_store::{ObjectStore, ObjectStoreExt, path::Path};
use sha2::{Digest, Sha256};

use super::{BackupObject, BackupObjectStatus, BackupStoreError, R2BackupStore};

const RECORD_LIMIT: usize = 1024 * 1024;
const VERIFY_RANGE_BYTES: u64 = 64 * 1024 * 1024;

impl R2BackupStore {
    /// 对象不存在返回 None；以不超过 64 MiB 的区间顺序回读，长度和完整摘要任一不符都冲突。
    pub async fn verify_existing(
        &self,
        key: &str,
        bytes: u64,
        sha256: &str,
    ) -> Result<Option<BackupObject>, BackupStoreError> {
        let path = Path::parse(key).map_err(|_| BackupStoreError::Configuration)?;
        let metadata = match self.store.head(&path).await {
            Ok(metadata) => metadata,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(BackupStoreError::from_store(error)),
        };
        if metadata.size != bytes {
            return Err(BackupStoreError::Conflict);
        }
        let mut digest = Sha256::new();
        let mut length = 0_u64;
        while length < bytes {
            let end = length.saturating_add(VERIFY_RANGE_BYTES).min(bytes);
            let chunk = match self.store.get_range(&path, length..end).await {
                Ok(chunk) => chunk,
                Err(object_store::Error::NotFound { .. }) => return Ok(None),
                Err(error) => return Err(BackupStoreError::from_store(error)),
            };
            if chunk.len() as u64 != end - length {
                return Err(BackupStoreError::Conflict);
            }
            length = length.checked_add(chunk.len() as u64).ok_or(BackupStoreError::Conflict)?;
            digest.update(&chunk);
        }
        if length != bytes || lower_hex(&digest.finalize()) != sha256 {
            return Err(BackupStoreError::Conflict);
        }
        Ok(Some(BackupObject {
            key: key.to_owned(),
            bytes,
            sha256: sha256.to_owned(),
            status: BackupObjectStatus::AlreadyPresent,
        }))
    }

    /// 有界读取无秘密的完成记录；不向调用方返回远端错误正文。
    pub async fn read_record(&self, key: &str) -> Result<Vec<u8>, BackupStoreError> {
        let path = Path::parse(key).map_err(|_| BackupStoreError::Configuration)?;
        let result = self.store.get(&path).await.map_err(|error| match error {
            object_store::Error::NotFound { .. } => BackupStoreError::RemoteRecord,
            error => BackupStoreError::from_store(error),
        })?;
        if result.meta.size > RECORD_LIMIT as u64 {
            return Err(BackupStoreError::RemoteRecord);
        }
        let mut stream = result.into_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = poll_fn(|context| stream.as_mut().poll_next(context)).await {
            let chunk = chunk.map_err(BackupStoreError::from_store)?;
            if bytes.len().saturating_add(chunk.len()) > RECORD_LIMIT {
                return Err(BackupStoreError::RemoteRecord);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    /// 枚举某月全部完成记录名称；调用方施加整次预算，并校验格式和引用的全部对象。
    pub async fn completion_records(&self, prefix: &str) -> Result<Vec<String>, BackupStoreError> {
        let path = Path::parse(prefix).map_err(|_| BackupStoreError::Configuration)?;
        let mut stream = self.store.list(Some(&path));
        let mut records = Vec::new();
        while let Some(metadata) = poll_fn(|context| stream.as_mut().poll_next(context)).await {
            let metadata = metadata.map_err(BackupStoreError::from_store)?;
            let key = metadata.location.as_ref();
            if key.starts_with(&format!("{prefix}/")) && key.ends_with("/completed.json") {
                records.push(key.to_owned());
            }
        }
        records.sort();
        Ok(records)
    }
}
