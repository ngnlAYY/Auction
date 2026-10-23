//! 结算履约 prepared delivery 写入与 activation 推进。

use super::mapping::*;

use super::*;

#[cfg(feature = "postgres")]
use std::{future::Future, pin::Pin};

#[cfg(feature = "postgres")]
type DeliveryTransactionFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DeliveryTransactionError>> + Send + 'a>>;

#[cfg(feature = "postgres")]
enum DeliveryTransactionError {
    Sqlx(sqlx::Error),
    Repository(auction_application::RepositoryError),
    Funding(auction_application::FundingError),
}

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for DeliveryTransactionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

#[cfg(feature = "postgres")]
impl From<auction_application::RepositoryError> for DeliveryTransactionError {
    fn from(error: auction_application::RepositoryError) -> Self {
        Self::Repository(error)
    }
}

#[cfg(feature = "postgres")]
impl DeliveryTransactionError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Funding(auction_application::FundingError::SerializationConflict))
            || matches!(
                self,
                Self::Sqlx(sqlx::Error::Database(error))
                    if matches!(error.code().as_deref(), Some("40001" | "40P01"))
            )
    }

    fn into_repository_error(self) -> auction_application::RepositoryError {
        match self {
            Self::Sqlx(error) => map_storage_error(error),
            Self::Repository(error) => error,
            Self::Funding(_) => repository_error_owned("履约业务锁或资金保护门不可用"),
        }
    }
}

/// 按旧版 `RunSerializable(..., 3)` 语义执行履约 owner 事务。
/// 只有 PostgreSQL 明确报告的序列化冲突（40001）或死锁（40P01）才重试；业务身份冲突、claim 失效
/// 和其他数据库错误直接返回。每次重试从头开启事务，但调用方传入的 delivery 与 audit 身份不变。
#[cfg(feature = "postgres")]
async fn run_serializable_delivery_transaction<T, F>(
    pool: &crate::database::TrackedPool<Postgres>,
    custody: Option<(&auction_application::SettlementInternalEvidenceSnapshot, std::time::Instant)>,
    mut operation: F,
) -> Result<T, auction_application::RepositoryError>
where
    T: Send,
    F: for<'a> FnMut(
            &'a mut sqlx::Transaction<'static, Postgres>,
            Option<&'a auction_application::SettlementInternalEvidenceSnapshot>,
        ) -> DeliveryTransactionFuture<'a, T>
        + Send,
{
    let deadline = custody.map(|(_, deadline)| deadline);
    let custody = custody.map(|(snapshot, _)| snapshot);
    let run = async {
        for attempt in 0..3 {
            let mut transaction = match pool.begin().await {
                Ok(transaction) => transaction,
                Err(error) => {
                    let error = DeliveryTransactionError::from(error);
                    if error.is_retryable() && attempt < 2 {
                        continue;
                    }
                    return Err(error.into_repository_error());
                }
            };

            if let Err(error) = sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .execute(&mut *transaction)
                .await
            {
                let error = DeliveryTransactionError::from(error);
                if error.is_retryable() && attempt < 2 {
                    let _ = transaction.rollback().await;
                    continue;
                }
                let _ = transaction.rollback().await;
                return Err(error.into_repository_error());
            }

            let operation_result = async {
                if let Some(custody) = custody {
                    let budget = (custody.expires_at - Utc::now()).num_milliseconds();
                    if budget <= 0 {
                        return Err(DeliveryTransactionError::Funding(
                            auction_application::FundingError::Unproven,
                        ));
                    }
                    // PostgreSQL 18 整事务预算包含锁等待与 COMMIT，重试不延长证据窗口。
                    sqlx::query("SELECT set_config('transaction_timeout',$1,true)")
                        .bind(format!("{budget}ms"))
                        .execute(&mut *transaction)
                        .await?;
                }
                let value = operation(&mut transaction, custody).await?;
                if let Some(custody) = custody {
                    crate::validate_mail_evidence_time(&mut transaction, custody)
                        .await
                        .map_err(super::internal::evidence_error)
                        .map_err(DeliveryTransactionError::Funding)?;
                }
                Ok(value)
            }
            .await;
            match operation_result {
                Ok(value) => match transaction.commit().await {
                    Ok(()) => return Ok(value),
                    Err(error) => {
                        let error = DeliveryTransactionError::from(error);
                        if error.is_retryable() && attempt < 2 {
                            continue;
                        }
                        return Err(error.into_repository_error());
                    }
                },
                Err(error) => {
                    let retryable = error.is_retryable();
                    let mapped = error.into_repository_error();
                    if retryable && attempt < 2 {
                        let _ = transaction.rollback().await;
                        continue;
                    }
                    let _ = transaction.rollback().await;
                    return Err(mapped);
                }
            }
        }

        Err(repository_error_owned("Serializable 履约事务重试状态异常"))
    };
    if let Some(deadline) = deadline {
        // 外层覆盖连接池排队；取消或超时由调用方尚未确认的 guard 撤销当前 owner。
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), run)
            .await
            .map_err(|_| repository_error_owned("退款库存证据提交预算已耗尽"))?
    } else {
        run.await
    }
}

#[cfg(feature = "postgres")]
pub(super) async fn prepare_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    preparation: &SettlementDeliveryPreparation,
    _prepared_at: DateTime<Utc>,
    custody: Option<(&auction_application::SettlementInternalEvidenceSnapshot, std::time::Instant)>,
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
    let expected_state_version = i64::try_from(claim.state_version)
        .map_err(|_| repository_error_owned("结算履约状态版本超出数据库范围"))?;
    let next_state_version = claim
        .state_version
        .checked_add(1)
        .ok_or_else(|| repository_error_owned("结算履约状态版本溢出"))?;
    let claim_snapshot = claim.clone();
    let preparation_snapshot = preparation.clone();
    let request_id_snapshot = preparation.prepare_command_id.to_string();
    let prepared_at = _prepared_at;
    let _inserted =
        run_serializable_delivery_transaction(pool, custody, move |transaction, custody| {
            let claim = claim_snapshot.clone();
            let preparation = preparation_snapshot.clone();
            let request_id = request_id_snapshot.clone();
            Box::pin(async move {
                super::internal::lock_legacy(
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
                .bind(preparation.request_hash.as_bytes().to_vec())
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
                    DeliveryTransactionError::from(repository_error_owned(
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
                    return Err(DeliveryTransactionError::from(repository_error_owned(
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
                         AND delivery_id IS NULL"#,
                )
                .bind(claim.obligation_id.as_uuid())
                .bind(preparation.delivery_id)
                .bind(preparation.prepare_command_id.as_uuid())
                .bind(preparation.request_hash.as_bytes().to_vec())
                .bind(expected_state_version)
                .bind(claim.claim_token)
                .bind(claim.claim_fencing)
                .execute(&mut **transaction)
                .await
                .map_err(DeliveryTransactionError::from)?;

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
                        DeliveryTransactionError::from(repository_error_owned("结算履约记录不存在"))
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
                                DeliveryTransactionError::from(repository_error_owned(
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
                        return Err(DeliveryTransactionError::from(repository_error_owned(
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
    let mut next_claim = claim.clone();
    next_claim.state = SettlementFulfillmentState::Prepared;
    next_claim.state_version = next_state_version;
    next_claim.delivery_id = Some(preparation.delivery_id);
    next_claim.prepare_command_id = Some(preparation.prepare_command_id);
    next_claim.prepare_request_hash = Some(preparation.request_hash);
    next_claim.delivery_snapshot = Some(SettlementFulfillmentDeliverySnapshot {
        id: preparation.delivery_id,
        status: auction_application::SettlementFulfillmentDeliveryStatus::Prepared,
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
    });
    Ok(SettlementFulfillmentResult {
        obligation_id: claim.obligation_id,
        delivery_id: preparation.delivery_id,
        prepare_command_id: preparation.prepare_command_id,
        next_claim,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn activate_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    activate_command_id: CommandId,
    activate_request_hash: auction_application::RequestHash,
    activated_at: DateTime<Utc>,
    _next_attempt_at: Option<DateTime<Utc>>,
    custody: Option<(&auction_application::SettlementInternalEvidenceSnapshot, std::time::Instant)>,
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
    let expected_state_version = i64::try_from(claim.state_version)
        .map_err(|_| repository_error_owned("结算履约状态版本超出数据库范围"))?;
    let next_state_version = claim
        .state_version
        .checked_add(1)
        .ok_or_else(|| repository_error_owned("结算履约状态版本溢出"))?;
    let mut next_delivery_snapshot = claim
        .delivery_snapshot
        .clone()
        .ok_or_else(|| repository_error_owned("prepared 履约缺少 delivery 快照"))?;
    next_delivery_snapshot.status =
        auction_application::SettlementFulfillmentDeliveryStatus::Pending;
    next_delivery_snapshot.activate_command_id = Some(activate_command_id);
    next_delivery_snapshot.activate_request_hash = Some(activate_request_hash);
    let request_id = activate_command_id.to_string();
    let claim_snapshot = claim.clone();
    let request_id_snapshot = request_id.clone();
    let _activated =
        run_serializable_delivery_transaction(pool, custody, move |transaction, custody| {
            let claim = claim_snapshot.clone();
            let request_id = request_id_snapshot.clone();
            Box::pin(async move {
                super::internal::lock_legacy(
                    transaction,
                    &claim,
                    custody.map(|evidence| (evidence, activate_command_id.as_uuid())),
                )
                .await
                .map_err(DeliveryTransactionError::Funding)?;
                let delivery = sqlx::query(
                    r#"SELECT status, activation_command_id, activation_request_digest
                   FROM public.deliveries
                   WHERE id = $1
                   FOR UPDATE"#,
                )
                .bind(delivery_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(DeliveryTransactionError::from)?
                .ok_or_else(|| {
                    DeliveryTransactionError::from(repository_error_owned(
                        "prepared delivery 不存在",
                    ))
                })?;
                let status: String =
                    delivery.try_get("status").map_err(DeliveryTransactionError::from)?;
                let existing_command: Option<Uuid> = delivery
                    .try_get("activation_command_id")
                    .map_err(DeliveryTransactionError::from)?;
                let existing_digest: Option<Vec<u8>> = delivery
                    .try_get("activation_request_digest")
                    .map_err(DeliveryTransactionError::from)?;
                let activated = match status.as_str() {
                    "prepared" => {
                        if existing_command.is_some() || existing_digest.is_some() {
                            return Err(DeliveryTransactionError::from(repository_error_owned(
                                "delivery activation identity 已被占用",
                            )));
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
                        .execute(&mut **transaction)
                        .await
                        .map_err(DeliveryTransactionError::from)?;
                        if updated.rows_affected() != 1 {
                            return Err(DeliveryTransactionError::from(repository_error_owned(
                                "delivery 激活状态已变化",
                            )));
                        }
                        true
                    }
                    "pending" | "claimed" | "sending" | "succeeded" | "failed"
                    | "manual_reconcile"
                        if existing_command == Some(activate_command_id.as_uuid())
                            && existing_digest.as_deref()
                                == Some(activate_request_hash.as_bytes().as_slice()) =>
                    {
                        false
                    }
                    _ => {
                        return Err(DeliveryTransactionError::from(repository_error_owned(
                            "delivery 当前状态不允许激活",
                        )));
                    }
                };

                // 激活后继续复用同一 claim 对账；领取期间 next_attempt_at 必须为空。
                // 兼容参数保留，下一共享窗口由对账延期在清 claim 的同一事务内持久化。
                let updated = sqlx::query(
                    r#"UPDATE public.settlement_obligation_fulfillments
                       SET execution_state = 'enqueued',
                           activate_command_id = $2,
                           activate_request_digest = $3,
                           state_version = state_version + 1,
                           next_attempt_at = NULL,
                           last_error_code = NULL,
                           updated_at = CURRENT_TIMESTAMP
                   WHERE obligation_id = $1
                         AND execution_state = 'prepared'
                         AND state_version = $4
                         AND claim_token = $5
                         AND claim_fencing = $6
                         AND claim_expires_at > clock_timestamp()
                         AND delivery_id = $7
                         AND prepare_command_id = $8
                         AND prepare_request_digest = $9"#,
                )
                .bind(claim.obligation_id.as_uuid())
                .bind(activate_command_id.as_uuid())
                .bind(activate_request_hash.as_bytes().to_vec())
                .bind(expected_state_version)
                .bind(claim.claim_token)
                .bind(claim.claim_fencing)
                .bind(delivery_id)
                .bind(prepare_command_id.as_uuid())
                .bind(prepare_request_hash.as_bytes().to_vec())
                .execute(&mut **transaction)
                .await
                .map_err(DeliveryTransactionError::from)?;
                if updated.rows_affected() == 0 {
                    let current = sqlx::query(
                        r#"SELECT execution_state, state_version, claim_token, claim_fencing,
                              delivery_id, activate_command_id, activate_request_digest
                       FROM public.settlement_obligation_fulfillments
                       WHERE obligation_id = $1
                       FOR UPDATE"#,
                    )
                    .bind(claim.obligation_id.as_uuid())
                    .fetch_optional(&mut **transaction)
                    .await
                    .map_err(DeliveryTransactionError::from)?
                    .ok_or_else(|| {
                        DeliveryTransactionError::from(repository_error_owned("结算履约记录不存在"))
                    })?;
                    let current_state: String = current
                        .try_get("execution_state")
                        .map_err(DeliveryTransactionError::from)?;
                    let current_state_version: i64 =
                        current.try_get("state_version").map_err(DeliveryTransactionError::from)?;
                    let current_claim_token: Option<Uuid> =
                        current.try_get("claim_token").map_err(DeliveryTransactionError::from)?;
                    let current_claim_fencing: i64 =
                        current.try_get("claim_fencing").map_err(DeliveryTransactionError::from)?;
                    let current_delivery: Option<Uuid> =
                        current.try_get("delivery_id").map_err(DeliveryTransactionError::from)?;
                    let current_command: Option<Uuid> = current
                        .try_get("activate_command_id")
                        .map_err(DeliveryTransactionError::from)?;
                    let current_digest: Option<Vec<u8>> = current
                        .try_get("activate_request_digest")
                        .map_err(DeliveryTransactionError::from)?;
                    if current_state != "enqueued"
                        || current_state_version
                            != i64::try_from(next_state_version).map_err(|_| {
                                DeliveryTransactionError::from(repository_error_owned(
                                    "结算履约状态版本超出数据库范围",
                                ))
                            })?
                        || current_claim_token != Some(claim.claim_token)
                        || current_claim_fencing != claim.claim_fencing
                        || current_delivery != Some(delivery_id)
                        || current_command != Some(activate_command_id.as_uuid())
                        || current_digest.as_deref()
                            != Some(activate_request_hash.as_bytes().as_slice())
                    {
                        return Err(DeliveryTransactionError::from(repository_error_owned(
                            "结算履约 activation claim 已失效或冲突",
                        )));
                    }
                }
                if activated {
                    append_delivery_audit(
                        &mut *transaction,
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
                    .map_err(DeliveryTransactionError::from)?;
                }
                Ok(activated)
            })
        })
        .await?;
    let mut next_claim = claim.clone();
    next_claim.state = SettlementFulfillmentState::Enqueued;
    next_claim.state_version = next_state_version;
    next_claim.activate_command_id = Some(activate_command_id);
    next_claim.activate_request_hash = Some(activate_request_hash);
    next_claim.delivery_snapshot = Some(next_delivery_snapshot);
    Ok(auction_application::SettlementFulfillmentActivationResult {
        obligation_id: claim.obligation_id,
        delivery_id,
        activate_command_id,
        next_claim,
    })
}
