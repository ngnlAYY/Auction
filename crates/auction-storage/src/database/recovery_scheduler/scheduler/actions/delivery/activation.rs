//! Recovery action 从 prepared 激活为 enqueued 的 PostgreSQL 写入。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(crate) async fn mark_action_enqueued_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    activate_command_id: CommandId,
    activate_request_hash: RequestHash,
    activated_at: DateTime<Utc>,
) -> Result<RecoveryActionClaim, auction_application::RepositoryError> {
    let delivery_id = claim
        .delivery_id
        .ok_or_else(|| repository_error("recovery prepared action 缺少 delivery 标识"))?;
    let prepare_command_id = claim
        .prepare_command_id
        .ok_or_else(|| repository_error("recovery prepared action 缺少 prepare command"))?;
    let prepare_request_hash = claim
        .prepare_request_hash
        .ok_or_else(|| repository_error("recovery prepared action 缺少 prepare digest"))?;
    let result = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = 'enqueued',
               activate_command_id = $1,
               activate_request_digest = $2,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $3
           WHERE id = $4
             AND recovery_id = $5
             AND execution_state = 'prepared'
             AND state_version = $6
             AND claim_token = $7
             AND claim_fencing = $8
             AND delivery_id = $9
             AND prepare_command_id = $10
             AND prepare_request_digest = $11"#,
    )
    .bind(activate_command_id.as_uuid())
    .bind(activate_request_hash.as_bytes().to_vec())
    .bind(activated_at)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .bind(delivery_id)
    .bind(prepare_command_id.as_uuid())
    .bind(prepare_request_hash.as_bytes().to_vec())
    .execute(pool)
    .await
    .map_err(map_storage_error)?;
    if result.rows_affected() != 1 {
        return Err(repository_error("recovery action activation fence 已失效"));
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
        execution_state: "enqueued".to_owned(),
        delivery_id: Some(delivery_id),
        prepare_command_id: Some(prepare_command_id),
        prepare_request_hash: Some(prepare_request_hash),
        activate_command_id: Some(activate_command_id),
        activate_request_hash: Some(activate_request_hash),
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
