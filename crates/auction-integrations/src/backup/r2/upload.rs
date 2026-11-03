//! 固定对象名上传：小对象原生 create-only，大对象先 HEAD 再原生分片；失败后统一回读对账。

use std::{fs::File, io::Read};

use auction_application::lower_hex;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions, path::Path};
use sha2::{Digest, Sha256};

use super::{BackupObject, BackupObjectStatus, BackupStoreError, R2BackupStore};

const MULTIPART_THRESHOLD: u64 = 128 * 1024 * 1024;
const MIN_PART_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PARTS: u64 = 10000;
const MAX_PART_BYTES: u64 = 5 * 1024 * 1024 * 1024;

impl R2BackupStore {
    /// 消费已经固定身份和摘要的本地密文；重试必须传入同一份密文字节。
    pub async fn upload_verified(
        &self,
        key: &str,
        file: File,
        bytes: u64,
        sha256: &str,
    ) -> Result<BackupObject, BackupStoreError> {
        if let Some(existing) = self.verify_existing(key, bytes, sha256).await? {
            return Ok(existing);
        }
        let path = Path::parse(key).map_err(|_| BackupStoreError::Configuration)?;
        let result = if bytes > MULTIPART_THRESHOLD {
            self.multipart(&path, file, bytes).await
        } else {
            let (_, payload) = read_chunk(file, bytes as usize).await?;
            if payload.len() as u64 != bytes || lower_hex(&Sha256::digest(&payload)) != sha256 {
                return Err(BackupStoreError::LocalRead);
            }
            self.store
                .put_opts(
                    &path,
                    payload.into(),
                    PutOptions { mode: PutMode::Create, ..Default::default() },
                )
                .await
                .map(|_| ())
                .map_err(BackupStoreError::from_store)
        };
        self.reconcile(key, bytes, sha256, result).await
    }

    /// 完成记录同样 create-only，且发布后完整回读；调用方持久化原字节后才能调用。
    pub async fn upload_record(
        &self,
        key: &str,
        payload: Vec<u8>,
    ) -> Result<BackupObject, BackupStoreError> {
        let bytes = payload.len() as u64;
        let sha256 = lower_hex(&Sha256::digest(&payload));
        if let Some(existing) = self.verify_existing(key, bytes, &sha256).await? {
            return Ok(existing);
        }
        let path = Path::parse(key).map_err(|_| BackupStoreError::Configuration)?;
        let result = self
            .store
            .put_opts(
                &path,
                payload.into(),
                PutOptions { mode: PutMode::Create, ..Default::default() },
            )
            .await
            .map(|_| ())
            .map_err(BackupStoreError::from_store);
        self.reconcile(key, bytes, &sha256, result).await
    }

    async fn reconcile(
        &self,
        key: &str,
        bytes: u64,
        sha256: &str,
        result: Result<(), BackupStoreError>,
    ) -> Result<BackupObject, BackupStoreError> {
        // 包括 409、412、完成响应丢失：只承认回读相同对象，不覆盖、删除或另取名称。
        match self.verify_existing(key, bytes, sha256).await? {
            Some(mut object) => {
                if result.is_ok() {
                    object.status = BackupObjectStatus::Uploaded;
                }
                Ok(object)
            }
            None => Err(result.err().unwrap_or(BackupStoreError::Transport)),
        }
    }

    async fn multipart(
        &self,
        path: &Path,
        mut file: File,
        bytes: u64,
    ) -> Result<(), BackupStoreError> {
        let part_bytes = MIN_PART_BYTES.max(bytes.div_ceil(MAX_PARTS));
        if part_bytes > MAX_PART_BYTES {
            return Err(BackupStoreError::Configuration);
        }
        // SDK 的公开分片完成接口没有条件写入。必须先启用 daily/monthly bucket lock，
        // 再依靠唯一 run_id、复用密文和完整回读共同保护不可变性；不改写 SDK 请求或签名。
        let mut upload =
            self.store.put_multipart(path).await.map_err(BackupStoreError::from_store)?;
        let result = async {
            let mut remaining = bytes;
            while remaining != 0 {
                let (next, first) = read_chunk(file, remaining.min(part_bytes) as usize).await?;
                file = next;
                if first.is_empty() {
                    return Err(BackupStoreError::LocalRead);
                }
                remaining -= first.len() as u64;
                let first = upload.put_part(first.into());
                if remaining == 0 {
                    first.await.map_err(BackupStoreError::from_store)?;
                } else {
                    let (next, second) =
                        read_chunk(file, remaining.min(part_bytes) as usize).await?;
                    file = next;
                    if second.is_empty() {
                        return Err(BackupStoreError::LocalRead);
                    }
                    remaining -= second.len() as u64;
                    let second = upload.put_part(second.into());
                    tokio::try_join!(first, second).map_err(BackupStoreError::from_store)?;
                }
            }
            upload.complete().await.map_err(BackupStoreError::from_store)?;
            Ok(())
        }
        .await;
        if result.is_err() {
            // 只回收未完成的分片会话，不调用对象删除；强杀遗留交给 bucket 的未完成分片生命周期。
            let _ = upload.abort().await;
        }
        result
    }
}

async fn read_chunk(mut file: File, limit: usize) -> Result<(File, Vec<u8>), BackupStoreError> {
    tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::with_capacity(limit);
        (&mut file)
            .take(limit as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| BackupStoreError::LocalRead)?;
        Ok((file, bytes))
    })
    .await
    .map_err(|_| BackupStoreError::LocalRead)?
}
