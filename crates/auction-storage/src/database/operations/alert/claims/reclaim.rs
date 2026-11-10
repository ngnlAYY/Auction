//! Operations alert 过期批次的租约回收事务。

use crate::database::storage_error_from;

use super::mapping::{digest_from_bytes, map_batch_items, request_hash_from_bytes};
use crate::database::operations::alert::delivery::terminalize_prepared_delivery;
use crate::database::operations::alert::{MAX_ATTEMPTS, storage_error};
use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertBatchClaim, OperationsAlertClaimRequest, OperationsAlertError, body_digest,
    canonical_summary,
};
use sqlx::Row;
use uuid::Uuid;

pub(super) async fn reclaim_batch(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    row: &sqlx::postgres::PgRow,
    request: &OperationsAlertClaimRequest,
) -> Result<Option<OperationsAlertBatchClaim>, ApplicationError> {
    let batch_id: Uuid = row
        .try_get("id")
        .map_err(|error| storage_error_from(error, "读取 operations alert 批次 ID 失败"))?;
    let attempt_count: i32 = row
        .try_get("attempt_count")
        .map_err(|error| storage_error_from(error, "读取 operations alert 尝试次数失败"))?;
    if attempt_count < 1 {
        return Err(storage_error("operations alert 尝试次数无效"));
    }
    if u32::try_from(attempt_count).ok().is_none_or(|value| value >= MAX_ATTEMPTS) {
        sqlx::query(
            "UPDATE public.operations_alert_batches
             SET status = 'failed', completed_at = $2, updated_at = $2,
                 claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL,
                 next_attempt_at = NULL, delivery_error_code = 'retry_exhausted'
             WHERE id = $1 AND status IN ('pending', 'claimed')",
        )
        .bind(batch_id)
        .bind(request.now)
        .execute(&mut **transaction)
        .await
        .map_err(|error| storage_error_from(error, "写入 operations alert 重试耗尽状态失败"))?;
        let delivery_id: Uuid = row
            .try_get("delivery_id")
            .map_err(|error| storage_error_from(error, "读取 operations alert delivery ID 失败"))?;
        terminalize_prepared_delivery(transaction, delivery_id, request.now, "retry_exhausted")
            .await?;
        return Ok(None);
    }
    let command_id: Uuid = row
        .try_get("command_id")
        .map_err(|error| storage_error_from(error, "读取 operations alert 命令 ID 失败"))?;
    let delivery_id: Uuid = row
        .try_get("delivery_id")
        .map_err(|error| storage_error_from(error, "读取 operations alert delivery ID 失败"))?;
    let request_digest: Vec<u8> = row
        .try_get("request_digest")
        .map_err(|error| storage_error_from(error, "读取 operations alert 请求摘要失败"))?;
    let body_digest_value: Vec<u8> = row
        .try_get("body_digest")
        .map_err(|error| storage_error_from(error, "读取 operations alert 正文摘要失败"))?;
    let claim_fencing: i64 = row
        .try_get("claim_fencing")
        .map_err(|error| storage_error_from(error, "读取 operations alert fencing 失败"))?;
    let delivery_target: Option<(String, serde_json::Value)> =
        sqlx::query_as("SELECT recipient, payload FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|error| {
                storage_error_from(error, "读取 operations alert delivery 目标失败")
            })?;
    let (recipient, payload) =
        delivery_target.ok_or_else(|| storage_error("operations alert delivery 不存在"))?;
    let payload_world = payload.get("world").and_then(serde_json::Value::as_str);
    if recipient != request.target.recipient
        || payload_world != Some(request.target.world_id.as_str())
    {
        sqlx::query(
            "UPDATE public.operations_alert_batches
             SET status = 'failed', claim_token = NULL, claimed_at = NULL,
                 claim_expires_at = NULL, next_attempt_at = NULL,
                 delivery_error_code = 'dependency_mismatch', completed_at = $2, updated_at = $2
             WHERE id = $1 AND status IN ('pending', 'claimed')",
        )
        .bind(batch_id)
        .bind(request.now)
        .execute(&mut **transaction)
        .await
        .map_err(|error| storage_error_from(error, "收口 operations alert target 变更批次失败"))?;
        terminalize_prepared_delivery(transaction, delivery_id, request.now, "dependency_mismatch")
            .await?;
        return Ok(None);
    }
    let item_rows = sqlx::query(
        "SELECT fingerprint, signal_version, ordinal, rendered_summary
         FROM public.operations_alert_batch_items
         WHERE batch_id = $1 ORDER BY ordinal",
    )
    .bind(batch_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 批次条目失败"))?;
    let items = map_batch_items(item_rows)?;
    let summary =
        canonical_summary(&items, auction_application::operations::MAX_OPERATIONS_ALERT_BODY_BYTES)
            .map_err(ApplicationError::from)?;
    let request_hash = request_hash_from_bytes(request_digest)?;
    let stored_body_digest = digest_from_bytes(body_digest_value)?;
    if body_digest(&summary) != stored_body_digest {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StoredResultCorrupt));
    }
    let next_attempt = u32::try_from(attempt_count)
        .map_err(|_| storage_error("operations alert 尝试次数无效"))?
        .checked_add(1)
        .ok_or_else(|| storage_error("operations alert 尝试次数溢出"))?;
    let claim_fencing = claim_fencing
        .checked_add(1)
        .ok_or_else(|| storage_error("operations alert fencing 溢出"))?;
    let claim_token = Uuid::new_v4();
    let updated = sqlx::query(
        "UPDATE public.operations_alert_batches
         SET status = 'claimed', claim_token = $2, claim_fencing = $3,
             claimed_at = $4, claim_expires_at = $5, next_attempt_at = NULL,
             attempt_count = $6, delivery_error_code = '', updated_at = $4
         WHERE id = $1 AND status IN ('pending', 'claimed')",
    )
    .bind(batch_id)
    .bind(claim_token)
    .bind(claim_fencing)
    .bind(request.now)
    .bind(request.lease_until)
    .bind(i32::try_from(next_attempt).map_err(|_| OperationsAlertError::Invalid)?)
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "更新 operations alert 重试租约失败"))?;
    if updated.rows_affected() != 1 {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim));
    }
    let claim = OperationsAlertBatchClaim {
        batch_id,
        command_id,
        delivery_id,
        request_hash,
        body_digest: stored_body_digest,
        summary,
        items,
        claim_token,
        claim_fencing,
        lease_until: request.lease_until,
        attempt_count: next_attempt,
    };
    claim
        .validate(auction_application::operations::MAX_OPERATIONS_ALERT_BODY_BYTES)
        .map_err(ApplicationError::from)?;
    Ok(Some(claim))
}
