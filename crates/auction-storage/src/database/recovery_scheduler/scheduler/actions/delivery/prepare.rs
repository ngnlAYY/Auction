//! Recovery action 从 pending 关联到 prepared 的 PostgreSQL 写入。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(crate) async fn link_action_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    delivery_id: Uuid,
    command_id: CommandId,
    request_hash: RequestHash,
    _linked_at: DateTime<Utc>,
) -> Result<RecoveryActionClaim, auction_application::RepositoryError> {
    let state_version = claim
        .state_version
        .checked_add(1)
        .ok_or_else(|| repository_error("recovery action 状态版本溢出"))?;
    let claim_snapshot = claim.clone();
    let request_digest = request_hash.as_bytes().to_vec();
    run_serializable_action_transaction(pool, move |transaction| {
        let claim = claim_snapshot.clone();
        let request_digest = request_digest.clone();
        Box::pin(async move {
            let result = sqlx::query(
                r#"UPDATE public.settlement_recovery_actions
                   SET execution_state = 'prepared',
                       delivery_id = $1,
                       prepare_command_id = $2,
                       prepare_request_digest = $3,
                       last_error_code = NULL,
                       next_attempt_at = NULL,
                       state_version = state_version + 1,
                       updated_at = clock_timestamp()
                   WHERE id = $4
                     AND recovery_id = $5
                     AND execution_state = 'pending'
                     AND state_version = $6
                     AND claim_token = $7
                     AND claim_fencing = $8
                     AND claim_expires_at > clock_timestamp()
                     AND delivery_id IS NULL"#,
            )
            .bind(delivery_id)
            .bind(command_id.as_uuid())
            .bind(&request_digest)
            .bind(claim.action_id.as_uuid())
            .bind(claim.recovery_id.as_uuid())
            .bind(claim.state_version)
            .bind(claim.claim_token)
            .bind(claim.fencing_version)
            .execute(&mut **transaction)
            .await
            .map_err(RecoveryActionTransactionError::from)?;
            if result.rows_affected() == 1 {
                return Ok(());
            }

            let current = sqlx::query(
                r#"SELECT recovery_id, execution_state, state_version, claim_token,
                          claim_fencing, delivery_id, prepare_command_id,
                          prepare_request_digest
                   FROM public.settlement_recovery_actions
                   WHERE id = $1
                   FOR UPDATE"#,
            )
            .bind(claim.action_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(RecoveryActionTransactionError::from)?
            .ok_or_else(|| repository_error("recovery action 不存在"))?;
            let current_recovery_id: Uuid =
                current.try_get("recovery_id").map_err(map_storage_error)?;
            let current_state: String =
                current.try_get("execution_state").map_err(map_storage_error)?;
            let current_state_version: i64 =
                current.try_get("state_version").map_err(map_storage_error)?;
            let current_claim_token: Option<Uuid> =
                current.try_get("claim_token").map_err(map_storage_error)?;
            let current_fencing: i64 =
                current.try_get("claim_fencing").map_err(map_storage_error)?;
            let current_delivery_id: Option<Uuid> =
                current.try_get("delivery_id").map_err(map_storage_error)?;
            let current_command_id: Option<Uuid> =
                current.try_get("prepare_command_id").map_err(map_storage_error)?;
            let current_digest: Option<Vec<u8>> =
                current.try_get("prepare_request_digest").map_err(map_storage_error)?;
            if current_recovery_id == claim.recovery_id.as_uuid()
                && current_state == "prepared"
                && current_state_version == state_version
                && current_claim_token == Some(claim.claim_token)
                && current_fencing == claim.fencing_version
                && current_delivery_id == Some(delivery_id)
                && current_command_id == Some(command_id.as_uuid())
                && current_digest.as_deref() == Some(request_digest.as_slice())
            {
                return Ok(());
            }
            if current_delivery_id.is_some_and(|value| value != delivery_id)
                || current_command_id.is_some_and(|value| value != command_id.as_uuid())
                || current_digest.as_deref().is_some_and(|value| value != request_digest)
            {
                return Err(repository_error("recovery action prepared identity 冲突").into());
            }
            Err(repository_error("recovery action delivery 关联 fence 已失效").into())
        })
    })
    .await?;
    Ok(RecoveryActionClaim {
        action_id: claim.action_id,
        recovery_id: claim.recovery_id,
        kind: claim.kind.clone(),
        world_id: claim.world_id.clone(),
        buyer_user_id: claim.buyer_user_id,
        recipient: claim.recipient.clone(),
        recipient_eligible: claim.recipient_eligible,
        recipient_state_version: claim.recipient_state_version,
        payment_reference: claim.payment_reference.clone(),
        source_external_mail_id: claim.source_external_mail_id.clone(),
        expected_credits: claim.expected_credits,
        expected_cod: claim.expected_cod,
        expected_attachment_ids: claim.expected_attachment_ids.clone(),
        execution_state: "prepared".to_owned(),
        delivery_id: Some(delivery_id),
        prepare_command_id: Some(command_id),
        prepare_request_hash: Some(request_hash),
        activate_command_id: None,
        activate_request_hash: None,
        state_version,
        claim_token: claim.claim_token,
        lease_until: claim.lease_until,
        fencing_version: claim.fencing_version,
        retry_count: claim.retry_count,
        worker_id: claim.worker_id.clone(),
        claimed_at: claim.claimed_at,
        created_at: claim.created_at,
    })
}
