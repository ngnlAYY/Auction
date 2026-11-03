//! Recovery action 的非投递完成、重试释放与人工复核收口。

#[cfg(feature = "postgres")]
use super::*;

/// 旧版 recovery owner 支持的三种 action 终态。
#[cfg(feature = "postgres")]
#[derive(Clone)]
pub(super) enum RecoveryTerminalState {
    Satisfied {
        proof_kind: String,
        proof_reference: String,
        proof_digest: [u8; 32],
        proof_observed_at: DateTime<Utc>,
    },
    Failed {
        failure_code: String,
    },
    ManualReconcile {
        failure_code: String,
    },
}

#[cfg(feature = "postgres")]
impl RecoveryTerminalState {
    fn state(&self) -> &'static str {
        match self {
            Self::Satisfied { .. } => "satisfied",
            Self::Failed { .. } => "failed",
            Self::ManualReconcile { .. } => "manual_reconcile",
        }
    }

    fn audit_action(&self) -> &'static str {
        match self {
            Self::Satisfied { .. } => "auction.recovery.satisfied",
            Self::Failed { .. } => "auction.recovery.failed",
            Self::ManualReconcile { .. } => "auction.recovery.manual_reconcile",
        }
    }

    fn failure_code(&self) -> Option<&str> {
        match self {
            Self::Satisfied { .. } => None,
            Self::Failed { failure_code } | Self::ManualReconcile { failure_code } => {
                Some(failure_code)
            }
        }
    }

    fn proof(&self) -> Option<(&str, &str, &[u8], DateTime<Utc>)> {
        match self {
            Self::Satisfied { proof_kind, proof_reference, proof_digest, proof_observed_at } => {
                Some((proof_kind, proof_reference, proof_digest.as_slice(), *proof_observed_at))
            }
            Self::Failed { .. } | Self::ManualReconcile { .. } => None,
        }
    }

    fn is_manual_reconcile(&self) -> bool {
        matches!(self, Self::ManualReconcile { .. })
    }
}

#[cfg(feature = "postgres")]
pub(crate) async fn mark_action_satisfied_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    proof_kind: &str,
    proof_reference: &str,
    proof_digest: [u8; 32],
    observed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let terminal = RecoveryTerminalState::Satisfied {
        proof_kind: proof_kind.to_owned(),
        proof_reference: proof_reference.to_owned(),
        proof_digest,
        proof_observed_at: observed_at,
    };
    run_terminal_action_postgres(pool, claim, terminal).await
}

#[cfg(feature = "postgres")]
pub(crate) async fn release_action_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    let claim_snapshot = claim.clone();
    let failure_code = failure_code.to_owned();
    run_serializable_action_transaction(pool, move |transaction| {
        let claim = claim_snapshot.clone();
        let failure_code = failure_code.clone();
        Box::pin(async move {
            lock_action(transaction, claim.action_id.as_uuid()).await?;
            let result = sqlx::query(
                r#"UPDATE public.settlement_recovery_actions
                   SET claim_token = NULL,
                       claimed_at = NULL,
                       claim_expires_at = NULL,
                       retry_count = retry_count + 1,
                       next_attempt_at = $1,
                       last_error_code = $2,
                       state_version = state_version + 1,
                       updated_at = statement_timestamp()
                   WHERE id = $3
                     AND recovery_id = $4
                     AND execution_state = $5
                     AND state_version = $6
                     AND claim_token = $7
                     AND claim_fencing = $8
                     AND claim_expires_at > clock_timestamp()"#,
            )
            .bind(retry_at)
            .bind(&failure_code)
            .bind(claim.action_id.as_uuid())
            .bind(claim.recovery_id.as_uuid())
            .bind(&claim.execution_state)
            .bind(claim.state_version)
            .bind(claim.claim_token)
            .bind(claim.fencing_version)
            .execute(&mut **transaction)
            .await
            .map_err(RecoveryActionTransactionError::from)?;
            if result.rows_affected() != 1 {
                return Err(repository_error("recovery action 释放 fence 已失效").into());
            }
            Ok(())
        })
    })
    .await
}

#[cfg(feature = "postgres")]
pub(crate) async fn manual_action_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    failure_code: &str,
) -> Result<(), auction_application::RepositoryError> {
    run_terminal_action_postgres(
        pool,
        claim,
        RecoveryTerminalState::ManualReconcile { failure_code: failure_code.to_owned() },
    )
    .await
}

#[cfg(feature = "postgres")]
async fn run_terminal_action_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    terminal: RecoveryTerminalState,
) -> Result<(), auction_application::RepositoryError> {
    // 审计 ID 在序列化重试轮次外生成，保证一次逻辑提交只有一个稳定审计身份。
    let audit_id = Uuid::new_v4();
    let claim_snapshot = claim.clone();
    run_serializable_action_transaction(pool, move |transaction| {
        let claim = claim_snapshot.clone();
        let terminal = terminal.clone();
        Box::pin(async move {
            complete_action_in_transaction(transaction.native(), &claim, &terminal, audit_id).await
        })
    })
    .await
}

/// 在调用方的 Serializable 事务中原子完成 action、父 recovery 投影和审计。
#[cfg(feature = "postgres")]
pub(super) async fn complete_action_in_transaction(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    claim: &RecoveryActionClaim,
    terminal: &RecoveryTerminalState,
    audit_id: Uuid,
) -> Result<(), RecoveryActionTransactionError> {
    // 父case必须在action之前锁定，与内部履约的 recovery→actions 前缀一致。
    sqlx::query("SELECT id FROM public.settlement_recoveries WHERE id=$1 FOR UPDATE")
        .bind(claim.recovery_id.as_uuid())
        .fetch_one(&mut **transaction)
        .await
        .map_err(RecoveryActionTransactionError::from)?;
    lock_action(transaction, claim.action_id.as_uuid()).await?;
    let (proof_kind, proof_reference, proof_digest, proof_observed_at) = terminal.proof().map_or(
        (None, None, None, None),
        |(kind, reference, digest, observed_at)| {
            (Some(kind), Some(reference), Some(digest), Some(observed_at))
        },
    );
    let updated = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = $1,
               last_error_code = $2,
               proof_kind = $3,
               proof_reference = $4,
               proof_digest = $5,
               proof_observed_at = $6,
               completed_at = statement_timestamp(),
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = statement_timestamp()
           WHERE id = $7
             AND recovery_id = $8
             AND execution_state = $9
             AND state_version = $10
             AND claim_token = $11
             AND claim_fencing = $12
             AND claim_expires_at > clock_timestamp()
           RETURNING updated_at"#,
    )
    .bind(terminal.state())
    .bind(terminal.failure_code())
    .bind(proof_kind)
    .bind(proof_reference)
    .bind(proof_digest)
    .bind(proof_observed_at)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(&claim.execution_state)
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(RecoveryActionTransactionError::from)?
    .ok_or_else(|| repository_error("recovery action 终态 claim 已失效"))?;
    let updated_at: DateTime<Utc> = updated.try_get("updated_at").map_err(map_storage_error)?;

    let projected = if terminal.state() == "satisfied" {
        sqlx::query(
            r#"UPDATE public.settlement_recoveries AS recovery
               SET custody_proof_kind = CASE
                       WHEN action.kind IN ('payment_reminder', 'buyer_blacklist')
                           THEN recovery.custody_proof_kind
                       ELSE $1
                   END,
                   custody_proof_reference = CASE
                       WHEN action.kind IN ('payment_reminder', 'buyer_blacklist')
                           THEN recovery.custody_proof_reference
                       ELSE $2
                   END,
                   custody_proof_digest = CASE
                       WHEN action.kind IN ('payment_reminder', 'buyer_blacklist')
                           THEN recovery.custody_proof_digest
                       ELSE $3
                   END,
                   custody_proof_observed_at = CASE
                       WHEN action.kind IN ('payment_reminder', 'buyer_blacklist')
                           THEN recovery.custody_proof_observed_at
                       ELSE $4
                   END,
                   blocking_code = NULL,
                   manual_reconcile_delivery_id = NULL,
                   state_version = recovery.state_version + 1,
                   updated_at = statement_timestamp()
               FROM public.settlement_recovery_actions AS action
               WHERE action.id = $5
                 AND action.recovery_id = recovery.id
                 AND action.execution_state = 'satisfied'
               RETURNING recovery.state_version"#,
        )
        .bind(proof_kind)
        .bind(proof_reference)
        .bind(proof_digest)
        .bind(proof_observed_at)
        .bind(claim.action_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(RecoveryActionTransactionError::from)?
    } else {
        sqlx::query(
            r#"UPDATE public.settlement_recoveries AS recovery
               SET blocking_code = $1,
                   manual_reconcile_delivery_id = CASE
                       WHEN $2::boolean THEN action.delivery_id
                       ELSE NULL
                   END,
                   state_version = recovery.state_version + 1,
                   updated_at = statement_timestamp()
               FROM public.settlement_recovery_actions AS action
               WHERE action.id = $3
                 AND action.recovery_id = recovery.id
                 AND action.execution_state = CASE
                       WHEN $2::boolean THEN 'manual_reconcile'
                       ELSE 'failed'
                   END
               RETURNING recovery.state_version"#,
        )
        .bind(terminal.failure_code())
        .bind(terminal.is_manual_reconcile())
        .bind(claim.action_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(RecoveryActionTransactionError::from)?
    }
    .ok_or_else(|| repository_error("recovery action 父 case 投影失败"))?;
    let projected_state_version: i64 =
        projected.try_get("state_version").map_err(map_storage_error)?;
    if projected_state_version <= 0 {
        return Err(repository_error("recovery 父 case 状态版本无效").into());
    }

    let metadata = serde_json::json!({
        "error_code": terminal.failure_code().unwrap_or(""),
        "fencing": claim.fencing_version,
        "proof_kind": proof_kind.unwrap_or(""),
        "recovery_id": claim.recovery_id.as_uuid().to_string(),
        "state": terminal.state(),
    });
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, source, metadata)
           VALUES ($1, $2, 'system', 'auction-recovery', $3,
                   'settlement_recovery_action', $4, 'success', 'worker', $5)"#,
    )
    .bind(audit_id)
    .bind(updated_at)
    .bind(terminal.audit_action())
    .bind(claim.action_id.as_uuid().to_string())
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .map_err(RecoveryActionTransactionError::from)?;
    Ok(())
}
