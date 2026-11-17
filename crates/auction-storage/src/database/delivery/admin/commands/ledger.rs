//! delivery Admin 命令账本：以 command ID 取事务锁统一并发入口，重放已提交结果，并识别同 ID
//! 不同请求的冲突与损坏的已存结果。

use auction_application::delivery::{
    DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminResult,
};
use serde_json::Value;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::super::mapping::{decode_result_payload, valid_snapshot};
use super::super::transaction::{DeliveryAdminTransactionError, PostgresDeliveryAdminTransaction};
use crate::database::TrackedPool;

/// 以 command ID 为 PostgreSQL 事务锁键，统一同一幂等身份的并发入口。
pub(super) async fn lock_command(
    transaction: &mut PostgresDeliveryAdminTransaction,
    command_id: Uuid,
) -> Result<(), DeliveryAdminTransactionError> {
    crate::database::lock_command_id(transaction.connection()?, command_id)
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)
}

/// 唯一冲突后只从另一笔完整 SERIALIZABLE 事务读取已提交账本，避免返回已回滚事务中的临时结果。
pub(super) async fn committed_command(
    pool: &TrackedPool<Postgres>,
    command: &DeliveryAdminCommand,
) -> Result<DeliveryAdminResult, DeliveryAdminTransactionError> {
    let mut transaction = PostgresDeliveryAdminTransaction::begin(pool).await?;
    let result = existing_command(&mut transaction, command)
        .await?
        .ok_or(DeliveryAdminError::Unavailable)?;
    transaction.commit().await?;
    Ok(result)
}

pub(super) async fn existing_command(
    transaction: &mut PostgresDeliveryAdminTransaction,
    command: &DeliveryAdminCommand,
) -> Result<Option<DeliveryAdminResult>, DeliveryAdminTransactionError> {
    let Some(row) = sqlx::query(
        "SELECT command_kind, request_digest, delivery_id, result_payload
         FROM public.delivery_admin_commands WHERE command_id = $1",
    )
    .bind(command.command_id)
    .fetch_optional(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?
    else {
        return Ok(None);
    };
    let kind: String = row.try_get("command_kind").map_err(DeliveryAdminTransactionError::Sqlx)?;
    let digest: Vec<u8> =
        row.try_get("request_digest").map_err(DeliveryAdminTransactionError::Sqlx)?;
    let delivery_id: Uuid =
        row.try_get("delivery_id").map_err(DeliveryAdminTransactionError::Sqlx)?;
    if kind != command.action.command_kind()
        || digest != command.request_hash.as_bytes()
        || delivery_id != command.delivery_id
    {
        return Err(DeliveryAdminTransactionError::Business(DeliveryAdminError::CommandConflict));
    }
    let payload: Value =
        row.try_get("result_payload").map_err(DeliveryAdminTransactionError::Sqlx)?;
    let result = decode_result_payload(payload, command.command_id)?;
    if result.command_id != command.command_id
        || result.snapshot.id != command.delivery_id
        || !valid_snapshot(&result.snapshot)
        || result.snapshot.status != command.action.transition().next_status
        || result.snapshot.fencing_version <= command.expected_fencing_version
    {
        return Err(DeliveryAdminTransactionError::Business(
            DeliveryAdminError::StoredResultCorrupt,
        ));
    }
    Ok(Some(result))
}
