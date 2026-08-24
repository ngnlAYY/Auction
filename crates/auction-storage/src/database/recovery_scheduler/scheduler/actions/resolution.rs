//! Recovery action 的非投递完成、重试释放与人工复核收口。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(crate) async fn mark_action_satisfied_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    proof_kind: &str,
    proof_reference: &str,
    observed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let digest = recovery_proof_digest(claim.action_id.as_uuid(), proof_reference);
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = 'satisfied',
               proof_kind = $1,
               proof_reference = $2,
               proof_digest = $3,
               proof_observed_at = $4,
               completed_at = $4,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $4
           WHERE id = $5
             AND recovery_id = $6
             AND kind = 'buyer_blacklist'
             AND execution_state = 'pending'
             AND state_version = $7
             AND claim_token = $8
             AND claim_fencing = $9"#,
    )
    .bind(proof_kind)
    .bind(proof_reference)
    .bind(digest.to_vec())
    .bind(observed_at)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action 非邮件完成 fence 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) async fn release_action_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = $1,
               last_error_code = $2,
               state_version = state_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $3
             AND recovery_id = $4
             AND execution_state = $5
             AND state_version = $6
             AND claim_token = $7
             AND claim_fencing = $8"#,
    )
    .bind(retry_at)
    .bind(failure_code)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(&claim.execution_state)
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action 释放 fence 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) async fn manual_action_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = 'manual_reconcile',
               last_error_code = $1,
               completed_at = CURRENT_TIMESTAMP,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $2
             AND recovery_id = $3
             AND execution_state = $4
             AND state_version = $5
             AND claim_token = $6
             AND claim_fencing = $7"#,
    )
    .bind(failure_code)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(&claim.execution_state)
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action 人工复核 fence 已失效"));
    }
    Ok(())
}
