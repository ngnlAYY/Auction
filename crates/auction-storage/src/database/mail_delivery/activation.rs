//! PostgreSQL recovery delivery 的 prepared → pending 两阶段激活。

use auction_application::{
    MailDeliveryActivation, MailDeliveryOrigin, MailDeliveryResult, RepositoryError,
};
use serde_json::json;
use uuid::Uuid;

use super::super::delivery_audit::{ACTIVATE, DeliveryAudit, append as append_delivery_audit};
use super::common::{map_storage_error, repository_error};

pub(super) async fn activate_postgres(
    pool: &super::super::TrackedPool<sqlx::Postgres>,
    activation: &MailDeliveryActivation,
) -> Result<MailDeliveryResult, RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let row = sqlx::query(
        "SELECT id, command_id, request_digest, origin_kind, status,
                activation_command_id, activation_request_digest
         FROM public.deliveries WHERE id = $1 FOR UPDATE",
    )
    .bind(activation.delivery_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("recovery delivery 不存在"))?;
    let command_id: Uuid = sqlx::Row::try_get(&row, "command_id").map_err(map_storage_error)?;
    let request_digest: Vec<u8> =
        sqlx::Row::try_get(&row, "request_digest").map_err(map_storage_error)?;
    let origin_kind: String = sqlx::Row::try_get(&row, "origin_kind").map_err(map_storage_error)?;
    let status: String = sqlx::Row::try_get(&row, "status").map_err(map_storage_error)?;
    let existing_activate_command_id: Option<Uuid> =
        sqlx::Row::try_get(&row, "activation_command_id").map_err(map_storage_error)?;
    let existing_activate_digest: Option<Vec<u8>> =
        sqlx::Row::try_get(&row, "activation_request_digest").map_err(map_storage_error)?;
    if command_id != activation.prepare_command_id.as_uuid()
        || request_digest != activation.prepare_request_hash.as_bytes()
        || origin_kind != MailDeliveryOrigin::SettlementRecoveryAction.as_str()
    {
        return Err(repository_error("recovery delivery prepare identity 冲突"));
    }
    let activated = match status.as_str() {
        "prepared" => {
            if existing_activate_command_id.is_some() || existing_activate_digest.is_some() {
                return Err(repository_error("recovery delivery activation identity 已被占用"));
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
            .execute(&mut *transaction)
            .await
            .map_err(map_storage_error)?;
            if updated.rows_affected() != 1 {
                return Err(repository_error("recovery delivery 激活状态已变化"));
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
        _ => return Err(repository_error("recovery delivery 状态无法激活")),
    };
    if activated {
        let request_id = activation.activate_command_id.to_string();
        append_delivery_audit(
            &mut transaction,
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
        .await
        .map_err(map_storage_error)?;
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(MailDeliveryResult { delivery_id: activation.delivery_id })
}
