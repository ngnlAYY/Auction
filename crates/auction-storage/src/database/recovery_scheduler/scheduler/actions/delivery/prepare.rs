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
    linked_at: DateTime<Utc>,
) -> Result<RecoveryActionClaim, auction_application::RepositoryError> {
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = 'prepared',
               delivery_id = $1,
               prepare_command_id = $2,
               prepare_request_digest = $3,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $4
           WHERE id = $5
             AND recovery_id = $6
             AND execution_state = 'pending'
             AND state_version > 0
             AND claim_token = $7
             AND claim_fencing = $8"#,
    )
    .bind(delivery_id)
    .bind(command_id.as_uuid())
    .bind(request_hash.as_bytes().to_vec())
    .bind(linked_at)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action delivery 关联 fence 已失效"));
    }
    let state_version = claim
        .state_version
        .checked_add(1)
        .ok_or_else(|| repository_error("recovery action 状态版本溢出"))?;
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
