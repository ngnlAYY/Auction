//! PostgreSQL 两阶段 delivery 的 prepared → pending 激活：恢复动作经独立事务激活，结算退件
//! Take 在其 owner 的业务事务内激活。

use auction_application::core::RepositoryError;
use auction_application::mail::{MailDeliveryActivation, MailDeliveryOrigin, MailDeliveryResult};
use serde_json::json;
use uuid::Uuid;

use super::errors::{MailDeliveryTransactionError, map_storage_error};
use crate::database::delivery::audit::{ACTIVATE, DeliveryAudit, append as append_delivery_audit};
use crate::database::repository_error;

pub(super) async fn activate_postgres(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    activation: &MailDeliveryActivation,
) -> Result<MailDeliveryResult, RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let result = activate_in_transaction(
        &mut transaction,
        activation,
        MailDeliveryOrigin::SettlementRecoveryAction,
    )
    .await
    .map_err(MailDeliveryTransactionError::into_repository)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

/// 由调用方持有业务事务：只接受指定来源的 prepared 投递，并以独立的激活身份推进为 pending；
/// 同一激活身份重放返回原结果，其他激活身份失败关闭。
pub(crate) async fn activate_in_transaction(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    activation: &MailDeliveryActivation,
    origin: MailDeliveryOrigin,
) -> Result<MailDeliveryResult, MailDeliveryTransactionError> {
    let row = sqlx::query(
        "SELECT id, command_id, request_digest, origin_kind, status,
                activation_command_id, activation_request_digest
         FROM public.deliveries WHERE id = $1 FOR UPDATE",
    )
    .bind(activation.delivery_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or_else(|| repository_error("两阶段 delivery 不存在"))?;
    let command_id: Uuid = sqlx::Row::try_get(&row, "command_id")?;
    let request_digest: Vec<u8> = sqlx::Row::try_get(&row, "request_digest")?;
    let origin_kind: String = sqlx::Row::try_get(&row, "origin_kind")?;
    let status: String = sqlx::Row::try_get(&row, "status")?;
    let existing_activate_command_id: Option<Uuid> =
        sqlx::Row::try_get(&row, "activation_command_id")?;
    let existing_activate_digest: Option<Vec<u8>> =
        sqlx::Row::try_get(&row, "activation_request_digest")?;
    if command_id != activation.prepare_command_id.as_uuid()
        || request_digest != activation.prepare_request_hash.as_bytes()
        || origin_kind != origin.as_str()
    {
        return Err(repository_error("两阶段 delivery prepare identity 冲突").into());
    }
    let activated = match status.as_str() {
        "prepared" => {
            if existing_activate_command_id.is_some() || existing_activate_digest.is_some() {
                return Err(repository_error("两阶段 delivery activation identity 已被占用").into());
            }
            let updated = sqlx::query(
                "UPDATE public.deliveries
                 SET status = 'pending', activation_command_id = $2,
                     activation_request_digest = $3, activated_at = $4, updated_at = $4
                 WHERE id = $1 AND status = 'prepared'",
            )
            .bind(activation.delivery_id)
            .bind(activation.activate_command_id.as_uuid())
            .bind(activation.activate_request_hash.as_bytes().as_slice())
            .bind(activation.activated_at)
            .execute(&mut **transaction)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(repository_error("两阶段 delivery 激活状态已变化").into());
            }
            true
        }
        "pending" | "claimed" | "sending" | "succeeded" | "failed" | "manual_reconcile"
            if existing_activate_command_id == Some(activation.activate_command_id.as_uuid())
                && existing_activate_digest.as_deref()
                    == Some(activation.activate_request_hash.as_bytes().as_slice()) =>
        {
            false
        }
        _ => return Err(repository_error("两阶段 delivery 状态无法激活").into()),
    };
    if activated {
        let request_id = activation.activate_command_id.to_string();
        append_delivery_audit(
            transaction,
            DeliveryAudit {
                occurred_at: activation.activated_at,
                actor_type: "system",
                actor_id: "delivery-service",
                action: ACTIVATE,
                delivery_id: activation.delivery_id,
                request_id: Some(&request_id),
                source: "system",
                metadata: json!({"status": "pending"}),
            },
        )
        .await?;
    }
    Ok(MailDeliveryResult { delivery_id: activation.delivery_id })
}
