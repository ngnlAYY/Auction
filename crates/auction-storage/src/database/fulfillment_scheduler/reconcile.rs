//! 结算履约外部证据收口、人工复核与失败退避。

use super::mapping::*;

use super::*;

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
    if claim.state != SettlementFulfillmentState::Enqueued
        || proof.kind.is_empty()
        || proof.kind != proof.kind.trim()
        || proof.reference.is_empty()
        || proof.reference != proof.reference.trim()
        || proof.digest == [0; 32]
        || proof.observed_at < claim.claimed_at
        || proof.observed_at > Utc::now()
        || claim.external_reference.as_deref() != Some(proof.reference.as_str())
        || !claim.external_reference_proven
    {
        return Err(repository_error_owned("结算履约满足证明不满足 fencing 或身份约束"));
    }
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'satisfied',
               proof_kind = $2,
               proof_reference = $3,
               proof_digest = $4,
               proof_observed_at = $5,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               completed_at = CURRENT_TIMESTAMP,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               last_error_code = NULL,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state = 'enqueued'
             AND claim_token = $6
             AND claim_fencing = $7
             AND delivery_id = $8"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(&proof.kind)
    .bind(&proof.reference)
    .bind(proof.digest.to_vec())
    .bind(proof.observed_at)
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .bind(delivery_id)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error_owned("结算履约满足证明的 claim 已失效"));
    }
    Ok(SettlementFulfillmentReconcileResult {
        obligation_id: claim.obligation_id,
        delivery_id,
        status: SettlementFulfillmentReconcileStatus::Satisfied,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn manual_reconcile_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'manual_reconcile',
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               completed_at = CURRENT_TIMESTAMP,
               state_version = state_version + 1,
               next_attempt_at = NULL,
               last_error_code = $2,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state IN ('pending', 'prepared', 'enqueued')
             AND claim_token = $3
             AND claim_fencing = $4"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(failure_code)
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error_owned("结算履约人工复核的 claim 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn fail_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    _safe_detail: &str,
) -> Result<(), auction_application::RepositoryError> {
    validate_claim_identity(claim)?;
    let result = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               state_version = state_version + 1,
               next_attempt_at = $2,
               last_error_code = $3,
               updated_at = CURRENT_TIMESTAMP
           WHERE obligation_id = $1
             AND execution_state IN ('pending', 'prepared', 'enqueued')
             AND claim_token = $4
             AND claim_fencing = $5"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(retry_at)
    .bind(failure_code)
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error_owned("结算履约失败记录的 claim 已失效"));
    }
    Ok(())
}
