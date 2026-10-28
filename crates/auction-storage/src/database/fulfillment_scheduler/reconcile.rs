//! 结算履约外部证据收口、人工复核与失败退避。

use super::mapping::*;

use super::*;

#[cfg(feature = "postgres")]
const SERIALIZABLE_RETRY_ATTEMPTS: usize = 3;

#[cfg(feature = "postgres")]
enum ReconcileTransactionError {
    Storage(sqlx::Error),
    Repository(auction_application::RepositoryError),
    Funding(auction_application::FundingError),
}

#[cfg(feature = "postgres")]
macro_rules! run_reconcile_transaction {
    ($pool:expr, $transaction:ident, $body:block) => {{
        let mut last_error = None;
        'retry: {
            for attempt in 0..SERIALIZABLE_RETRY_ATTEMPTS {
                let mut $transaction =
                    match $pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await {
                        Ok(transaction) => transaction,
                        Err(error) => {
                            if crate::database::is_serialization_conflict(&error)
                                && attempt + 1 < SERIALIZABLE_RETRY_ATTEMPTS
                            {
                                last_error = Some(error);
                                continue;
                            }
                            return Err(map_storage_error(error));
                        }
                    };

                let operation_result = (async $body).await;
                match operation_result {
                    Ok(value) => match $transaction.commit().await {
                        Ok(()) => break 'retry Ok(value),
                        Err(error) => {
                            if crate::database::is_serialization_conflict(&error)
                                && attempt + 1 < SERIALIZABLE_RETRY_ATTEMPTS
                            {
                                last_error = Some(error);
                                continue;
                            }
                            return Err(map_storage_error(error));
                        }
                    }
                    Err(ReconcileTransactionError::Funding(error)) => {
                        if matches!(error, auction_application::FundingError::SerializationConflict)
                            && attempt + 1 < SERIALIZABLE_RETRY_ATTEMPTS {
                            continue;
                        }
                        return Err(repository_error_owned("履约资金保护门不可用"));
                    }
                    Err(ReconcileTransactionError::Repository(error)) => return Err(error),
                    Err(ReconcileTransactionError::Storage(error)) => {
                        if crate::database::is_serialization_conflict(&error)
                            && attempt + 1 < SERIALIZABLE_RETRY_ATTEMPTS
                        {
                            last_error = Some(error);
                            continue;
                        }
                        return Err(map_storage_error(error));
                    }
                }
            }

            match last_error {
                Some(error) => Err(map_storage_error(error)),
                None => Err(repository_error_owned("结算履约 Serializable 重试未返回结果")),
            }
        }
    }};
}

#[cfg(feature = "postgres")]
fn fulfillment_state_name(state: SettlementFulfillmentState) -> &'static str {
    match state {
        SettlementFulfillmentState::Pending => "pending",
        SettlementFulfillmentState::Prepared => "prepared",
        SettlementFulfillmentState::Enqueued => "enqueued",
    }
}

#[cfg(feature = "postgres")]
pub(super) async fn satisfy_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    proof: &SettlementFulfillmentProof,
) -> Result<SettlementFulfillmentReconcileResult, auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    let Some(delivery_id) = claim.delivery_id else {
        return Err(repository_error_owned("结算履约满足证明缺少 delivery identity"));
    };
    let Some(delivery) = claim.delivery_snapshot.as_ref() else {
        return Err(repository_error_owned("结算履约满足证明缺少 delivery 快照"));
    };
    if claim.state != SettlementFulfillmentState::Enqueued
        || claim.operation != SettlementFulfillmentOperation::Send
        || delivery.id != delivery_id
        || delivery.status != auction_application::SettlementFulfillmentDeliveryStatus::Succeeded
        || !delivery.external_reference_proven
        || proof.kind != "mooglemail_sent_collected"
        || proof.reference.is_empty()
        || proof.reference != proof.reference.trim()
        || proof.digest == [0; 32]
        || proof.observed_at < claim.claimed_at
        || proof.observed_at > Utc::now()
    {
        return Err(repository_error_owned("结算履约满足证明不满足 fencing 或身份约束"));
    }
    let state_version = i64::try_from(claim.state_version)
        .map_err(|_| repository_error_owned("结算履约状态版本超出数据库范围"))?;
    // 审计 ID 在重试轮次外生成，保证一次逻辑提交只对应一个审计身份。
    let audit_id = Uuid::new_v4();
    run_reconcile_transaction!(pool, transaction, {
        let protected = super::internal::lock_legacy(&mut transaction, claim, None)
            .await
            .map_err(ReconcileTransactionError::Funding)?;
        if protected
            && matches!(
                claim.kind,
                SettlementObligationKind::WinnerEquipmentCod
                    | SettlementObligationKind::WinnerPaymentCarrierCod
            )
        {
            return Err(ReconcileTransactionError::Funding(
                auction_application::FundingError::Unproven,
            ));
        }
        let updated = sqlx::query(
            r#"UPDATE public.settlement_obligation_fulfillments
                   SET execution_state = 'satisfied',
                       proof_kind = $1,
                       proof_reference = $2,
                       proof_digest = $3,
                       proof_observed_at = $4,
                       claim_token = NULL,
                       claimed_at = NULL,
                       claim_expires_at = NULL,
                       completed_at = statement_timestamp(),
                       state_version = state_version + 1,
                       next_attempt_at = NULL,
                       last_error_code = NULL,
                       updated_at = statement_timestamp()
                   WHERE obligation_id = $5
                     AND execution_state = $6
                     AND state_version = $7
                     AND claim_token = $8
                     AND claim_fencing = $9
                     AND delivery_id = $10
                     AND delivery_operation = 'send'
                     AND claim_expires_at > clock_timestamp()
                   RETURNING updated_at"#,
        )
        .bind(&proof.kind)
        .bind(&proof.reference)
        .bind(proof.digest.to_vec())
        .bind(proof.observed_at)
        .bind(claim.obligation_id.as_uuid())
        .bind(fulfillment_state_name(claim.state))
        .bind(state_version)
        .bind(claim.claim_token)
        .bind(claim.claim_fencing)
        .bind(delivery_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(ReconcileTransactionError::Storage)?;
        let updated_at: DateTime<Utc> = updated
            .ok_or_else(|| {
                ReconcileTransactionError::Repository(repository_error_owned(
                    "结算履约满足证明的 claim 已失效",
                ))
            })?
            .try_get("updated_at")
            .map_err(ReconcileTransactionError::Storage)?;
        // 满足事实与其直接依赖必须同事务提交，避免依赖在中间态被 Worker 领取或永久阻塞。
        let activated = sqlx::query(
            r#"UPDATE public.settlement_obligations
                   SET business_state = 'ready',
                       state_version = state_version + 1,
                       updated_at = clock_timestamp()
                   WHERE prerequisite_obligation_id = $1
                     AND business_state = 'blocked'"#,
        )
        .bind(claim.obligation_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(ReconcileTransactionError::Storage)?;
        append_terminal_audit(
            &mut transaction,
            audit_id,
            claim,
            "satisfied",
            "auction.fulfillment.satisfied",
            None,
            Some(&proof.kind),
            activated.rows_affected(),
            updated_at,
        )
        .await
        .map_err(ReconcileTransactionError::Storage)?;
        Ok(())
    })?;
    Ok(SettlementFulfillmentReconcileResult {
        obligation_id: claim.obligation_id,
        delivery_id,
        status: SettlementFulfillmentReconcileStatus::Satisfied,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn manual_reconcile_in_transaction(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    obligation_id: Uuid,
    execution_state: &str,
    state_version: u64,
    claim_token: Uuid,
    claim_fencing: i64,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    let state_version = i64::try_from(state_version)
        .map_err(|_| repository_error_owned("结算履约状态版本超出数据库范围"))?;
    let audit_id = Uuid::new_v4();
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
               SET execution_state = 'manual_reconcile',
                   claim_token = NULL,
                   claimed_at = NULL,
                   claim_expires_at = NULL,
                   completed_at = statement_timestamp(),
                   state_version = state_version + 1,
                   next_attempt_at = NULL,
                   last_error_code = $1,
                   updated_at = statement_timestamp()
               WHERE obligation_id = $2
                 AND execution_state = $3
                 AND state_version = $4
                 AND claim_token = $5
                 AND claim_fencing = $6
                 AND claim_expires_at > clock_timestamp()
               RETURNING updated_at"#,
    )
    .bind(failure_code)
    .bind(obligation_id)
    .bind(execution_state)
    .bind(state_version)
    .bind(claim_token)
    .bind(claim_fencing)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let updated_at: DateTime<Utc> = updated
        .ok_or_else(|| repository_error_owned("结算履约损坏上下文的 claim 已失效"))?
        .try_get("updated_at")
        .map_err(map_storage_error)?;
    append_manual_reconcile_audit(
        transaction,
        audit_id,
        obligation_id,
        claim_fencing,
        failure_code,
        updated_at,
    )
    .await
    .map_err(map_storage_error)
}

#[cfg(feature = "postgres")]
async fn finalize_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    failure_code: &str,
    execution_state: &str,
    audit_action: &str,
    invalid_claim_message: &str,
) -> Result<(), auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    let state_version = i64::try_from(claim.state_version)
        .map_err(|_| repository_error_owned("结算履约状态版本超出数据库范围"))?;
    let audit_id = Uuid::new_v4();
    run_reconcile_transaction!(pool, transaction, {
        lock_fulfillment(&mut transaction, claim.obligation_id.as_uuid())
            .await
            .map_err(ReconcileTransactionError::Storage)?;
        let updated = sqlx::query(
            r#"UPDATE public.settlement_obligation_fulfillments
                   SET execution_state = $1,
                       claim_token = NULL,
                       claimed_at = NULL,
                       claim_expires_at = NULL,
                       completed_at = statement_timestamp(),
                       state_version = state_version + 1,
                       next_attempt_at = NULL,
                       last_error_code = $2,
                       updated_at = statement_timestamp()
                   WHERE obligation_id = $3
                     AND execution_state = $4
                     AND state_version = $5
                     AND claim_token = $6
                     AND claim_fencing = $7
                     AND claim_expires_at > clock_timestamp()
                   RETURNING updated_at"#,
        )
        .bind(execution_state)
        .bind(failure_code)
        .bind(claim.obligation_id.as_uuid())
        .bind(fulfillment_state_name(claim.state))
        .bind(state_version)
        .bind(claim.claim_token)
        .bind(claim.claim_fencing)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(ReconcileTransactionError::Storage)?;
        let updated_at: DateTime<Utc> = updated
            .ok_or_else(|| {
                ReconcileTransactionError::Repository(repository_error_owned(invalid_claim_message))
            })?
            .try_get("updated_at")
            .map_err(ReconcileTransactionError::Storage)?;
        append_terminal_audit(
            &mut transaction,
            audit_id,
            claim,
            execution_state,
            audit_action,
            Some(failure_code),
            None,
            0,
            updated_at,
        )
        .await
        .map_err(ReconcileTransactionError::Storage)?;
        Ok(())
    })?;
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn manual_reconcile_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    finalize_postgres(
        pool,
        claim,
        failure_code,
        "manual_reconcile",
        "auction.fulfillment.manual_reconcile",
        "结算履约人工复核的 claim 已失效",
    )
    .await
}

#[cfg(feature = "postgres")]
async fn append_manual_reconcile_audit(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    audit_id: Uuid,
    obligation_id: Uuid,
    claim_fencing: i64,
    failure_code: &str,
    occurred_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let metadata = serde_json::json!({
        "state": "manual_reconcile",
        "fencing": claim_fencing,
        "activated_dependent_count": 0,
        "error_code": failure_code,
    });
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, source, metadata)
           VALUES ($1, $2, 'system', 'auction-fulfillment',
                   'auction.fulfillment.manual_reconcile',
                   'settlement_obligation', $3, 'success', 'worker', $4)"#,
    )
    .bind(audit_id)
    .bind(occurred_at)
    .bind(obligation_id.to_string())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .map(|_| ())
}

#[cfg(feature = "postgres")]
pub(super) async fn mark_failed_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    finalize_postgres(
        pool,
        claim,
        failure_code,
        "failed",
        "auction.fulfillment.failed",
        "结算履约确定失败的 claim 已失效",
    )
    .await
}

#[cfg(feature = "postgres")]
pub(super) async fn fail_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    retry_delay: chrono::Duration,
    failure_code: &str,
    _safe_detail: &str,
) -> Result<(), auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    let retry_microseconds = retry_delay
        .num_microseconds()
        .filter(|value| *value > 0)
        .ok_or_else(|| repository_error_owned("结算履约重试间隔无效"))?;
    let state_version = i64::try_from(claim.state_version)
        .map_err(|_| repository_error_owned("结算履约状态版本超出数据库范围"))?;
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    lock_fulfillment(&mut transaction, claim.obligation_id.as_uuid())
        .await
        .map_err(map_storage_error)?;
    let result = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               retry_count = retry_count + 1,
               state_version = state_version + 1,
               next_attempt_at = clock_timestamp() + $2::bigint * interval '1 microsecond',
               last_error_code = $3,
               updated_at = clock_timestamp()
           WHERE obligation_id = $1
             AND execution_state = $4
             AND state_version = $5
             AND claim_token = $6
             AND claim_fencing = $7
             AND claim_expires_at > clock_timestamp()"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(retry_microseconds)
    .bind(failure_code)
    .bind(fulfillment_state_name(claim.state))
    .bind(state_version)
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("结算履约失败记录的 claim 已失效"));
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

/// 取得 execution 行锁后再校验 TTL，避免锁等待前求值的时钟放过过期 claim。
#[cfg(feature = "postgres")]
async fn lock_fulfillment(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    obligation_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT obligation_id FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1 FOR UPDATE",
    )
    .bind(obligation_id)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(feature = "postgres")]
#[allow(clippy::too_many_arguments)]
async fn append_terminal_audit(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    audit_id: Uuid,
    claim: &SettlementFulfillmentClaim,
    state: &str,
    action: &str,
    error_code: Option<&str>,
    proof_kind: Option<&str>,
    activated_dependent_count: u64,
    occurred_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let mut object = serde_json::Map::new();
    object.insert("state".to_owned(), state.into());
    object.insert("fencing".to_owned(), claim.claim_fencing.into());
    object.insert("activated_dependent_count".to_owned(), activated_dependent_count.into());
    if let Some(error_code) = error_code {
        object.insert("error_code".to_owned(), error_code.into());
    }
    if let Some(proof_kind) = proof_kind {
        object.insert("proof_kind".to_owned(), proof_kind.into());
    }
    let metadata = serde_json::Value::Object(object);
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, source, metadata)
           VALUES ($1, $2, 'system', 'auction-fulfillment', $3,
                   'settlement_obligation', $4, 'success', 'worker', $5)"#,
    )
    .bind(audit_id)
    .bind(occurred_at)
    .bind(action)
    .bind(claim.obligation_id.to_string())
    .bind(metadata)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
