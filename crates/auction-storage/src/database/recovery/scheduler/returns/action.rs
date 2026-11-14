//! Recovery 下一步 action 的幂等写入。

use crate::database::recovery::scheduler::mapping::map_storage_error;
use crate::database::recovery::scheduler::repository_error;
use crate::database::recovery::scheduler::returns::model::RecoveryActionInsert;
use auction_domain::{CommandId, deterministic_uuid};
use sqlx::Row;
use uuid::Uuid;

pub(super) async fn insert_recovery_action(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    spec: RecoveryActionInsert<'_>,
) -> Result<(), auction_application::core::RepositoryError> {
    let action_id = deterministic_uuid(
        CommandId::from_uuid(spec.recovery_id),
        &format!("recovery-action:{}", spec.kind),
        1,
    );
    // 重复退回时同一个 reclaim action 会被重新使用：重置回 pending、清空旧投递与证明，
    // 并提升 fencing，避免上一轮的 claim 或终态继续生效。
    let query = if spec.reset_existing {
        r#"INSERT INTO public.settlement_recovery_actions
               (id, recovery_id, kind, ordinal, execution_state, state_version,
                source_mail_id, expected_credits, expected_cod, expected_attachment_ids,
                reason, created_at, updated_at)
           VALUES ($1, $2, $3, $4, 'pending', 1, $5, $6, $7, $8, $9, $10, $10)
           ON CONFLICT (recovery_id, kind) DO UPDATE SET
               ordinal = EXCLUDED.ordinal,
               execution_state = 'pending',
               state_version = settlement_recovery_actions.state_version + 1,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               claim_fencing = settlement_recovery_actions.claim_fencing + 1,
               retry_count = 0,
               next_attempt_at = NULL,
               source_mail_id = EXCLUDED.source_mail_id,
               expected_credits = EXCLUDED.expected_credits,
               expected_cod = EXCLUDED.expected_cod,
               expected_attachment_ids = EXCLUDED.expected_attachment_ids,
               reason = EXCLUDED.reason,
               delivery_id = NULL,
               prepare_command_id = NULL,
               prepare_request_digest = NULL,
               activate_command_id = NULL,
               activate_request_digest = NULL,
               proof_kind = NULL,
               proof_reference = NULL,
               proof_digest = NULL,
               proof_observed_at = NULL,
               last_error_code = NULL,
               completed_at = NULL,
               updated_at = EXCLUDED.updated_at"#
    } else {
        r#"INSERT INTO public.settlement_recovery_actions
               (id, recovery_id, kind, ordinal, execution_state, state_version,
                source_mail_id, expected_credits, expected_cod, expected_attachment_ids,
                reason, created_at, updated_at)
           VALUES ($1, $2, $3, $4, 'pending', 1, $5, $6, $7, $8, $9, $10, $10)
           ON CONFLICT (recovery_id, kind) DO NOTHING"#
    };
    sqlx::query(query)
        .bind(action_id)
        .bind(spec.recovery_id)
        .bind(spec.kind)
        .bind(spec.ordinal)
        .bind(spec.source_mail_id)
        .bind(spec.expected_credits)
        .bind(spec.expected_cod)
        .bind(spec.expected_attachment_ids)
        .bind(spec.reason)
        .bind(spec.now)
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;

    let row = sqlx::query(
        r#"SELECT id, ordinal, source_mail_id, expected_credits, expected_cod,
                  expected_attachment_ids
           FROM public.settlement_recovery_actions
           WHERE recovery_id = $1 AND kind = $2
           FOR UPDATE"#,
    )
    .bind(spec.recovery_id)
    .bind(spec.kind)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let stored_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
    let stored_ordinal: i32 = row.try_get("ordinal").map_err(map_storage_error)?;
    let stored_source: Option<Uuid> = row.try_get("source_mail_id").map_err(map_storage_error)?;
    let stored_credits: i64 = row.try_get("expected_credits").map_err(map_storage_error)?;
    let stored_cod: i64 = row.try_get("expected_cod").map_err(map_storage_error)?;
    let stored_attachments: Vec<String> =
        row.try_get("expected_attachment_ids").map_err(map_storage_error)?;
    if stored_id != action_id
        || stored_ordinal != spec.ordinal
        || stored_source != spec.source_mail_id
        || stored_credits != spec.expected_credits
        || stored_cod != spec.expected_cod
        || stored_attachments != spec.expected_attachment_ids
    {
        return Err(repository_error("recovery action 证据内容冲突"));
    }
    Ok(())
}
