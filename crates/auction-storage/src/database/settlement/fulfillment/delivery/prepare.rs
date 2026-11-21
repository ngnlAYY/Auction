//! 结算履约 prepared delivery 的首次写入：冻结意图、投递与审计同事务提交。

use auction_application::settlement::{
    SettlementDeliveryPreparation, SettlementFulfillmentClaim,
    SettlementFulfillmentDeliverySnapshot, SettlementFulfillmentResult, SettlementFulfillmentState,
};
use auction_time::Timestamp;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::{DeliveryTransactionError, run_serializable_delivery_transaction};
use crate::database::delivery::audit::{DeliveryAudit, PREPARE, append as append_delivery_audit};
use crate::database::repository_error;
use crate::database::settlement::fulfillment::mapping::validate_claim_identity;

pub(in crate::database) async fn prepare_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
    claim: &SettlementFulfillmentClaim,
    preparation: &SettlementDeliveryPreparation,
    prepared_at: Timestamp,
    custody: Option<(
        &auction_application::settlement::SettlementInternalEvidenceSnapshot,
        std::time::Instant,
    )>,
) -> Result<SettlementFulfillmentResult, auction_application::core::RepositoryError> {
    validate_claim_identity(claim)?;
    if preparation.delivery_id.is_nil()
        || preparation.prepare_command_id.is_nil()
        || preparation.request_hash.as_bytes() == [0; 32]
        || !preparation.payload.is_object()
        || preparation.recipient != claim.recipient
    {
        return Err(repository_error("结算履约 prepared delivery 身份无效"));
    }
    let expected_state_version = i64::try_from(claim.state_version)
        .map_err(|_| repository_error("结算履约状态版本超出数据库范围"))?;
    let next_state_version = claim
        .state_version
        .checked_add(1)
        .ok_or_else(|| repository_error("结算履约状态版本溢出"))?;
    let dispatch_no = i32::try_from(claim.dispatch_no)
        .ok()
        .filter(|value| *value >= 1)
        .ok_or_else(|| repository_error("结算履约派发编号无效"))?;
    let claim_snapshot = claim.clone();
    let preparation_snapshot = preparation.clone();
    let request_id_snapshot = preparation.prepare_command_id.to_string();
    let inserted =
        run_serializable_delivery_transaction(pool, custody, move |transaction, custody| {
            let claim = claim_snapshot.clone();
            let preparation = preparation_snapshot.clone();
            let request_id = request_id_snapshot.clone();
            Box::pin(async move {
                super::super::internal::lock_legacy(
                    transaction,
                    &claim,
                    custody.map(|evidence| (evidence, preparation.prepare_command_id.as_uuid())),
                )
                .await
                .map_err(DeliveryTransactionError::Funding)?;
                let inserted = sqlx::query(
                    r#"INSERT INTO public.deliveries (
                       id, command_id, request_digest, origin_kind, origin_id,
                       recipient, payload, status, created_at, updated_at
                   ) VALUES ($1, $2, $3, 'settlement', $4, $5, $6, 'prepared', $7, $7)
                   ON CONFLICT (command_id) DO NOTHING"#,
                )
                .bind(preparation.delivery_id)
                .bind(preparation.prepare_command_id.as_uuid())
                .bind(preparation.request_hash.as_bytes().as_slice())
                .bind(claim.obligation_id.to_string())
                .bind(&preparation.recipient)
                .bind(&preparation.payload)
                .bind(prepared_at)
                .execute(&mut **transaction)
                .await
                .map_err(DeliveryTransactionError::from)?;
                let inserted = inserted.rows_affected() == 1;

                let delivery = sqlx::query(
                    r#"SELECT id, request_digest, origin_kind, origin_id, recipient, payload, status
                   FROM public.deliveries
                   WHERE command_id = $1
                   FOR UPDATE"#,
                )
                .bind(preparation.prepare_command_id.as_uuid())
                .fetch_optional(&mut **transaction)
                .await
                .map_err(DeliveryTransactionError::from)?
                .ok_or_else(|| {
                    DeliveryTransactionError::from(repository_error(
                        "prepared delivery 写入后无法读取",
                    ))
                })?;
                let stored_id: Uuid =
                    delivery.try_get("id").map_err(DeliveryTransactionError::from)?;
                let stored_digest: Vec<u8> =
                    delivery.try_get("request_digest").map_err(DeliveryTransactionError::from)?;
                let stored_origin_kind: String =
                    delivery.try_get("origin_kind").map_err(DeliveryTransactionError::from)?;
                let stored_origin_id: String =
                    delivery.try_get("origin_id").map_err(DeliveryTransactionError::from)?;
                let stored_recipient: String =
                    delivery.try_get("recipient").map_err(DeliveryTransactionError::from)?;
                let stored_payload: serde_json::Value =
                    delivery.try_get("payload").map_err(DeliveryTransactionError::from)?;
                let stored_status: String =
                    delivery.try_get("status").map_err(DeliveryTransactionError::from)?;
                if stored_id != preparation.delivery_id
                    || stored_digest != preparation.request_hash.as_bytes()
                    || stored_origin_kind != "settlement"
                    || stored_origin_id != claim.obligation_id.to_string()
                    || stored_recipient != preparation.recipient
                    || stored_payload != preparation.payload
                    || stored_status != "prepared"
                {
                    return Err(DeliveryTransactionError::from(repository_error(
                        "prepared delivery 命令账本冲突",
                    )));
                }

                let updated = sqlx::query(
                    r#"UPDATE public.settlement_obligation_fulfillments
                       SET execution_state = 'prepared',
                           delivery_id = $2,
                           prepare_command_id = $3,
                           prepare_request_digest = $4,
                           state_version = state_version + 1,
                           next_attempt_at = NULL,
                           last_error_code = NULL,
                           updated_at = CURRENT_TIMESTAMP
                       WHERE obligation_id = $1
                         AND execution_state = 'pending'
                         AND state_version = $5
                         AND claim_token = $6
                         AND claim_fencing = $7
                         AND claim_expires_at > clock_timestamp()
                         AND delivery_id IS NULL
                         AND dispatch_no = $8"#,
                )
                .bind(claim.obligation_id.as_uuid())
                .bind(preparation.delivery_id)
                .bind(preparation.prepare_command_id.as_uuid())
                .bind(preparation.request_hash.as_bytes().as_slice())
                .bind(expected_state_version)
                .bind(claim.claim_token)
                .bind(claim.claim_fencing)
                .bind(dispatch_no)
                .execute(&mut **transaction)
                .await
                .map_err(DeliveryTransactionError::from)?;
                record_dispatch(
                    transaction,
                    &claim,
                    dispatch_no,
                    preparation.delivery_id,
                    prepared_at,
                )
                .await?;

                if updated.rows_affected() == 0 {
                    let current = sqlx::query(
                        r#"SELECT execution_state, state_version, claim_token, claim_fencing,
                              delivery_id, prepare_command_id, prepare_request_digest
                       FROM public.settlement_obligation_fulfillments
                       WHERE obligation_id = $1
                       FOR UPDATE"#,
                    )
                    .bind(claim.obligation_id.as_uuid())
                    .fetch_optional(&mut **transaction)
                    .await
                    .map_err(DeliveryTransactionError::from)?
                    .ok_or_else(|| {
                        DeliveryTransactionError::from(repository_error("结算履约记录不存在"))
                    })?;
                    let state: String = current
                        .try_get("execution_state")
                        .map_err(DeliveryTransactionError::from)?;
                    let current_state_version: i64 =
                        current.try_get("state_version").map_err(DeliveryTransactionError::from)?;
                    let current_claim_token: Option<Uuid> =
                        current.try_get("claim_token").map_err(DeliveryTransactionError::from)?;
                    let current_claim_fencing: i64 =
                        current.try_get("claim_fencing").map_err(DeliveryTransactionError::from)?;
                    let current_delivery_id: Option<Uuid> =
                        current.try_get("delivery_id").map_err(DeliveryTransactionError::from)?;
                    let current_command_id: Option<Uuid> = current
                        .try_get("prepare_command_id")
                        .map_err(DeliveryTransactionError::from)?;
                    let current_digest: Option<Vec<u8>> = current
                        .try_get("prepare_request_digest")
                        .map_err(DeliveryTransactionError::from)?;
                    if state != "prepared"
                        || current_state_version
                            != i64::try_from(next_state_version).map_err(|_| {
                                DeliveryTransactionError::from(repository_error(
                                    "结算履约状态版本超出数据库范围",
                                ))
                            })?
                        || current_claim_token != Some(claim.claim_token)
                        || current_claim_fencing != claim.claim_fencing
                        || current_delivery_id != Some(preparation.delivery_id)
                        || current_command_id != Some(preparation.prepare_command_id.as_uuid())
                        || current_digest.as_deref()
                            != Some(preparation.request_hash.as_bytes().as_slice())
                    {
                        return Err(DeliveryTransactionError::from(repository_error(
                            "结算履约 claim 已失效或结果冲突",
                        )));
                    }
                }
                if inserted {
                    append_delivery_audit(
                        transaction,
                        DeliveryAudit {
                            occurred_at: prepared_at,
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
                    .map_err(DeliveryTransactionError::from)?;
                }
                Ok(inserted)
            })
        })
        .await?;
    // 只在事务提交且确实新建 outbox 行后计一次创建；序列化重试、回滚与幂等重放都不计。
    if inserted
        && let Some(kind) =
            auction_application::notification_metrics::NotificationKind::from_delivery(
                "settlement",
                preparation.payload.get("operation").and_then(serde_json::Value::as_str),
                preparation.payload.get("template").and_then(serde_json::Value::as_str),
            )
    {
        notifications.observe_stage(
            kind,
            auction_application::notification_metrics::NotificationStage::Created,
        );
    }
    let mut next_claim = claim.clone();
    next_claim.state = SettlementFulfillmentState::Prepared;
    next_claim.state_version = next_state_version;
    next_claim.delivery_id = Some(preparation.delivery_id);
    next_claim.prepare_command_id = Some(preparation.prepare_command_id);
    next_claim.prepare_request_hash = Some(preparation.request_hash);
    next_claim.delivery_snapshot = Some(SettlementFulfillmentDeliverySnapshot {
        id: preparation.delivery_id,
        status: auction_application::settlement::SettlementFulfillmentDeliveryStatus::Prepared,
        prepare_command_id: preparation.prepare_command_id,
        prepare_request_hash: preparation.request_hash,
        activate_command_id: None,
        activate_request_hash: None,
        origin_kind: "settlement".to_owned(),
        origin_id: claim.obligation_id.to_string(),
        recipient: preparation.recipient.clone(),
        payload: preparation.payload.clone(),
        external_reference: None,
        external_reference_proven: false,
        failure_code: None,
        completed_at: None,
    });
    Ok(SettlementFulfillmentResult {
        obligation_id: claim.obligation_id,
        delivery_id: preparation.delivery_id,
        prepare_command_id: preparation.prepare_command_id,
        next_claim,
    })
}

/// 派发历史只追加：首寄与补寄都在准备事务内登记，补寄必须引用授权这一编号的退件案件（否则
/// CHECK 失败关闭），且只寄给该案件绑定的被退回派发的冻结收件人：收件人改名后的补寄需要另行
/// 准入，不在这里放行。同一派发重放时历史行必须指向同一投递。
async fn record_dispatch(
    transaction: &mut crate::database::TrackedTransaction<'static, Postgres>,
    claim: &SettlementFulfillmentClaim,
    dispatch_no: i32,
    delivery_id: Uuid,
    prepared_at: Timestamp,
) -> Result<(), DeliveryTransactionError> {
    if dispatch_no > 1 {
        let original: Option<String> = sqlx::query_scalar(
            r#"SELECT returned.recipient
               FROM public.settlement_return_cases AS authorizing
               JOIN public.deliveries AS returned ON returned.id = authorizing.delivery_id
               WHERE authorizing.obligation_id = $1 AND authorizing.resend_dispatch_no = $2"#,
        )
        .bind(claim.obligation_id.as_uuid())
        .bind(dispatch_no)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(DeliveryTransactionError::from)?;
        if original.as_deref() != Some(claim.recipient.as_str()) {
            return Err(DeliveryTransactionError::from(repository_error(
                "补寄收件人与原冻结收件人不符",
            )));
        }
    }
    sqlx::query(
        r#"INSERT INTO public.settlement_fulfillment_dispatches
               (obligation_id, dispatch_no, delivery_id, authorized_by, recorded_at)
           SELECT $1, $2, $3,
                  CASE WHEN $2 = 1 THEN NULL ELSE (
                      SELECT authorizing.return_mail_id
                      FROM public.settlement_return_cases AS authorizing
                      WHERE authorizing.obligation_id = $1
                        AND authorizing.resend_dispatch_no = $2
                  ) END,
                  $4
           ON CONFLICT (obligation_id, dispatch_no) DO NOTHING"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(dispatch_no)
    .bind(delivery_id)
    .bind(prepared_at)
    .execute(&mut **transaction)
    .await
    .map_err(DeliveryTransactionError::from)?;
    let recorded: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT delivery_id FROM public.settlement_fulfillment_dispatches
           WHERE obligation_id = $1 AND dispatch_no = $2"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(dispatch_no)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(DeliveryTransactionError::from)?;
    if recorded != Some(delivery_id) {
        return Err(DeliveryTransactionError::from(repository_error("结算履约派发历史冲突")));
    }
    Ok(())
}
