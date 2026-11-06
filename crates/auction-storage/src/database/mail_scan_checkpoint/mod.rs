//! Sent / Read Mail 扫描检查点的 PostgreSQL 持久化。
//!
//! 检查点只服务增量前缀；写入失败、作用域不匹配或行损坏都不能阻断发送与轮询。
//! Inbox 的权威游标仍在 `mail_poll_state`，本表不参与任何完成判定。

use auction_application::core::ApplicationError;
use auction_application::mail::{
    MailScanCheckpoint, MailScanCheckpointFuture, MailScanCheckpointKey, MailScanCheckpointStore,
    MailScanStream, MailScanWatermark,
};
use sqlx::{Postgres, Row};

use super::DatabaseStore;
use crate::database::{TrackedPool, storage_error, storage_error_from};

fn stream_label(stream: MailScanStream) -> Option<&'static str> {
    match stream {
        MailScanStream::Sent => Some("sent"),
        MailScanStream::ReadMail => Some("read_mail"),
        MailScanStream::Inbox => None,
    }
}

fn validate_key(key: &MailScanCheckpointKey) -> Result<(), ApplicationError> {
    if key.validate() {
        Ok(())
    } else {
        Err(storage_error("MoogleMail 扫描检查点作用域无效"))
    }
}

fn validate_checkpoint(checkpoint: &MailScanCheckpoint) -> Result<(), ApplicationError> {
    if !checkpoint.validate() {
        return Err(storage_error("MoogleMail 扫描检查点内容无效"));
    }
    if checkpoint.watermark.is_none() && checkpoint.last_full_scan_at.is_none() {
        return Err(storage_error("MoogleMail 扫描检查点没有任何可持久状态"));
    }
    Ok(())
}

async fn load(
    pool: &TrackedPool<Postgres>,
    key: &MailScanCheckpointKey,
) -> Result<Option<MailScanCheckpoint>, ApplicationError> {
    let stream = stream_label(key.stream)
        .ok_or_else(|| storage_error("MoogleMail 扫描检查点不接受 Inbox 流"))?;
    let row = sqlx::query(
        "SELECT watermark_mail_id, watermark_observed_at, last_full_scan_at
         FROM public.mail_scan_checkpoints
         WHERE credential_id = $1
           AND world_id = $2
           AND stream = $3
           AND credential_version = $4
           AND endpoint_scope = $5",
    )
    .bind(key.credential_id)
    .bind(&key.world_id)
    .bind(stream)
    .bind(key.credential_version)
    .bind(&key.endpoint_scope)
    .fetch_optional(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 MoogleMail 扫描检查点失败"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mail_id: Option<String> = row
        .try_get("watermark_mail_id")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 扫描水位 MID 失败"))?;
    let observed_at = row
        .try_get("watermark_observed_at")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 扫描水位时间失败"))?;
    let last_full_scan_at = row
        .try_get("last_full_scan_at")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 完整校验时间失败"))?;
    let watermark = match (mail_id, observed_at) {
        (None, None) => None,
        (Some(mail_id), Some(observed_at)) => Some(MailScanWatermark { mail_id, observed_at }),
        _ => return Err(storage_error("MoogleMail 扫描检查点水位字段不完整")),
    };
    let checkpoint = MailScanCheckpoint { watermark, last_full_scan_at };
    if !checkpoint.validate() {
        return Err(storage_error("MoogleMail 扫描检查点行内容无效"));
    }
    Ok(Some(checkpoint))
}

async fn save(
    pool: &TrackedPool<Postgres>,
    key: &MailScanCheckpointKey,
    checkpoint: &MailScanCheckpoint,
) -> Result<(), ApplicationError> {
    let stream = stream_label(key.stream)
        .ok_or_else(|| storage_error("MoogleMail 扫描检查点不接受 Inbox 流"))?;
    let (mail_id, observed_at) = match &checkpoint.watermark {
        Some(watermark) => (Some(watermark.mail_id.as_str()), Some(watermark.observed_at)),
        None => (None, None),
    };
    sqlx::query(
        "INSERT INTO public.mail_scan_checkpoints
            (credential_id, world_id, stream, credential_version, endpoint_scope,
             watermark_mail_id, watermark_observed_at, last_full_scan_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, transaction_timestamp())
         ON CONFLICT (credential_id, world_id, stream) DO UPDATE
         SET credential_version = EXCLUDED.credential_version,
             endpoint_scope = EXCLUDED.endpoint_scope,
             watermark_mail_id = EXCLUDED.watermark_mail_id,
             watermark_observed_at = EXCLUDED.watermark_observed_at,
             last_full_scan_at = EXCLUDED.last_full_scan_at,
             updated_at = EXCLUDED.updated_at",
    )
    .bind(key.credential_id)
    .bind(&key.world_id)
    .bind(stream)
    .bind(key.credential_version)
    .bind(&key.endpoint_scope)
    .bind(mail_id)
    .bind(observed_at)
    .bind(checkpoint.last_full_scan_at)
    .execute(pool)
    .await
    .map_err(|error| storage_error_from(error, "写入 MoogleMail 扫描检查点失败"))?;
    Ok(())
}

impl MailScanCheckpointStore for DatabaseStore {
    fn load<'a>(
        &'a self,
        key: &'a MailScanCheckpointKey,
    ) -> MailScanCheckpointFuture<'a, Option<MailScanCheckpoint>> {
        Box::pin(async move {
            validate_key(key)?;
            let Self::Postgres(store) = self;
            load(store.pool(), key).await
        })
    }

    fn save<'a>(
        &'a self,
        key: &'a MailScanCheckpointKey,
        checkpoint: &'a MailScanCheckpoint,
    ) -> MailScanCheckpointFuture<'a, ()> {
        Box::pin(async move {
            validate_key(key)?;
            validate_checkpoint(checkpoint)?;
            let Self::Postgres(store) = self;
            save(store.pool(), key, checkpoint).await
        })
    }
}
