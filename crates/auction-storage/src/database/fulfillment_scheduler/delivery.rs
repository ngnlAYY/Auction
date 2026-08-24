//! 结算履约 prepared delivery 写入与 activation 推进。

use super::mapping::*;

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn prepare_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    preparation: &SettlementDeliveryPreparation,
    _prepared_at: DateTime<Utc>,
) -> Result<SettlementFulfillmentResult, auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    if preparation.delivery_id.is_nil()
        || preparation.prepare_command_id.is_nil()
        || preparation.request_hash.as_bytes() == [0; 32]
        || !preparation.payload.is_object()
        || preparation.recipient != claim.recipient
    {
        return Err(repository_error_owned("结算履约 prepared delivery 身份无效"));
    }

    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let inserted = sqlx::query(
        r#"INSERT INTO public.deliveries (
               id, command_id, request_digest, origin_kind, origin_id,
               recipient, payload, status, created_at, updated_at
           ) VALUES ($1, $2, $3, 'settlement', $4, $5, $6, 'prepared', $7, $7)
           ON CONFLICT (command_id) DO NOTHING"#,
    )
    .bind(preparation.delivery_id)
    .bind(preparation.prepare_command_id.as_uuid())
    .bind(preparation.request_hash.as_bytes().to_vec())
    .bind(claim.obligation_id.to_string())
    .bind(&preparation.recipient)
    .bind(&preparation.payload)
    .bind(_prepared_at)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    let delivery = sqlx::query(
        r#"SELECT id, request_digest, origin_kind, origin_id, recipient, payload, status
           FROM public.deliveries
           WHERE command_id = $1
           FOR UPDATE"#,
    )
    .bind(preparation.prepare_command_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error_owned("prepared delivery 写入后无法读取"))?;
    let stored_id: Uuid = delivery.try_get("id").map_err(map_storage_error)?;
    let stored_digest: Vec<u8> = delivery.try_get("request_digest").map_err(map_storage_error)?;
    let stored_origin_kind: String = delivery.try_get("origin_kind").map_err(map_storage_error)?;
    let stored_origin_id: String = delivery.try_get("origin_id").map_err(map_storage_error)?;
    let stored_recipient: String = delivery.try_get("recipient").map_err(map_storage_error)?;
    let stored_payload: serde_json::Value =
        delivery.try_get("payload").map_err(map_storage_error)?;
    let stored_status: String = delivery.try_get("status").map_err(map_storage_error)?;
    if stored_id != preparation.delivery_id
        || stored_digest != preparation.request_hash.as_bytes()
        || stored_origin_kind != "settlement"
        || stored_origin_id != claim.obligation_id.to_string()
        || stored_recipient != preparation.recipient
        || stored_payload != preparation.payload
        || !matches!(stored_status.as_str(), "prepared" | "pending")
    {
        return Err(repository_error_owned("prepared delivery 命令账本冲突"));
    }

    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'prepared',
               delivery_id = $2,
               prepare_command_id = $3,
               prepare_request_digest = $4,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               last_error_code = NULL,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state = 'pending'
             AND claim_token = $5
             AND claim_fencing = $6
             AND delivery_id IS NULL"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(preparation.delivery_id)
    .bind(preparation.prepare_command_id.as_uuid())
    .bind(preparation.request_hash.as_bytes().to_vec())
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;

    if updated.rows_affected() == 0 {
        let current = sqlx::query(
            r#"SELECT execution_state, delivery_id, prepare_command_id, prepare_request_digest
               FROM public.settlement_obligation_fulfillments
               WHERE obligation_id = $1
               FOR UPDATE"#,
        )
        .bind(claim.obligation_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_storage_error)?
        .ok_or_else(|| repository_error_owned("结算履约记录不存在"))?;
        let state: String = current.try_get("execution_state").map_err(map_storage_error)?;
        let current_delivery_id: Option<Uuid> =
            current.try_get("delivery_id").map_err(map_storage_error)?;
        let current_command_id: Option<Uuid> =
            current.try_get("prepare_command_id").map_err(map_storage_error)?;
        let current_digest: Option<Vec<u8>> =
            current.try_get("prepare_request_digest").map_err(map_storage_error)?;
        if state != "prepared"
            || current_delivery_id != Some(preparation.delivery_id)
            || current_command_id != Some(preparation.prepare_command_id.as_uuid())
            || current_digest.as_deref() != Some(preparation.request_hash.as_bytes().as_slice())
        {
            return Err(repository_error_owned("结算履约 claim 已失效或结果冲突"));
        }
    }
    if inserted.rows_affected() == 1 {
        let request_id = preparation.prepare_command_id.to_string();
        append_delivery_audit(
            &mut transaction,
            DeliveryAudit {
                occurred_at: _prepared_at,
                actor_type: "system",
                actor_id: "delivery-service",
                action: PREPARE,
                delivery_id: preparation.delivery_id,
                request_id: Some(&request_id),
                source: "system",
                metadata: serde_json::json!({
                    "origin_kind": "settlement",
                    "status": "prepared",
                }),
            },
        )
        .await
        .map_err(map_storage_error)?;
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(SettlementFulfillmentResult {
        obligation_id: claim.obligation_id,
        delivery_id: preparation.delivery_id,
        prepare_command_id: preparation.prepare_command_id,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn activate_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    activate_command_id: CommandId,
    activate_request_hash: auction_application::RequestHash,
    activated_at: DateTime<Utc>,
) -> Result<
    auction_application::SettlementFulfillmentActivationResult,
    auction_application::RepositoryError,
> {
    validate_claim_identity(claim)?;
    let (Some(delivery_id), Some(prepare_command_id), Some(prepare_request_hash)) =
        (claim.delivery_id, claim.prepare_command_id, claim.prepare_request_hash)
    else {
        return Err(repository_error_owned("prepared 履约缺少 delivery prepare identity"));
    };
    if claim.state != SettlementFulfillmentState::Prepared
        || activate_command_id.is_nil()
        || activate_request_hash.as_bytes() == [0; 32]
    {
        return Err(repository_error_owned("结算履约 activation identity 无效"));
    }

    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let delivery = sqlx::query(
        r#"SELECT status, activation_command_id, activation_request_digest
           FROM public.deliveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(delivery_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error_owned("prepared delivery 不存在"))?;
    let status: String = delivery.try_get("status").map_err(map_storage_error)?;
    let existing_command: Option<Uuid> =
        delivery.try_get("activation_command_id").map_err(map_storage_error)?;
    let existing_digest: Option<Vec<u8>> =
        delivery.try_get("activation_request_digest").map_err(map_storage_error)?;
    let activated = match status.as_str() {
        "prepared" => {
            if existing_command.is_some() || existing_digest.is_some() {
                return Err(repository_error_owned("delivery activation identity 已被占用"));
            }
            let updated = sqlx::query(
                r#"UPDATE public.deliveries
                   SET status = 'pending',
                       activation_command_id = $2,
                       activation_request_digest = $3,
                       activated_at = $4,
                       updated_at = CURRENT_TIMESTAMP
                   WHERE id = $1 AND status = 'prepared'"#,
            )
            .bind(delivery_id)
            .bind(activate_command_id.as_uuid())
            .bind(activate_request_hash.as_bytes().to_vec())
            .bind(activated_at)
            .execute(&mut *transaction)
            .await
            .map_err(map_storage_error)?;
            if updated.rows_affected() != 1 {
                return Err(repository_error_owned("delivery 激活状态已变化"));
            }
            true
        }
        "pending" | "claimed" | "sending" | "succeeded" | "failed" | "manual_reconcile"
            if existing_command == Some(activate_command_id.as_uuid())
                && existing_digest.as_deref()
                    == Some(activate_request_hash.as_bytes().as_slice()) =>
        {
            false
        }
        _ => return Err(repository_error_owned("delivery 当前状态不允许激活")),
    };

    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'enqueued',
               activate_command_id = $2,
               activate_request_digest = $3,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               last_error_code = NULL,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state = 'prepared'
             AND claim_token = $4
             AND claim_fencing = $5
             AND delivery_id = $6
             AND prepare_command_id = $7
             AND prepare_request_digest = $8"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(activate_command_id.as_uuid())
    .bind(activate_request_hash.as_bytes().to_vec())
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .bind(delivery_id)
    .bind(prepare_command_id.as_uuid())
    .bind(prepare_request_hash.as_bytes().to_vec())
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() == 0 {
        let current = sqlx::query(
            r#"SELECT execution_state, delivery_id, activate_command_id,
                      activate_request_digest
               FROM public.settlement_obligation_fulfillments
               WHERE obligation_id = $1
               FOR UPDATE"#,
        )
        .bind(claim.obligation_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_storage_error)?
        .ok_or_else(|| repository_error_owned("结算履约记录不存在"))?;
        let current_state: String =
            current.try_get("execution_state").map_err(map_storage_error)?;
        let current_delivery: Option<Uuid> =
            current.try_get("delivery_id").map_err(map_storage_error)?;
        let current_command: Option<Uuid> =
            current.try_get("activate_command_id").map_err(map_storage_error)?;
        let current_digest: Option<Vec<u8>> =
            current.try_get("activate_request_digest").map_err(map_storage_error)?;
        if current_state != "enqueued"
            || current_delivery != Some(delivery_id)
            || current_command != Some(activate_command_id.as_uuid())
            || current_digest.as_deref() != Some(activate_request_hash.as_bytes().as_slice())
        {
            return Err(repository_error_owned("结算履约 activation claim 已失效或冲突"));
        }
    }
    if activated {
        let request_id = activate_command_id.to_string();
        append_delivery_audit(
            &mut transaction,
            DeliveryAudit {
                occurred_at: activated_at,
                actor_type: "system",
                actor_id: "delivery-service",
                action: ACTIVATE,
                delivery_id,
                request_id: Some(&request_id),
                source: "system",
                metadata: serde_json::json!({"status": "pending"}),
            },
        )
        .await
        .map_err(map_storage_error)?;
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(auction_application::SettlementFulfillmentActivationResult {
        obligation_id: claim.obligation_id,
        delivery_id,
        activate_command_id,
    })
}
