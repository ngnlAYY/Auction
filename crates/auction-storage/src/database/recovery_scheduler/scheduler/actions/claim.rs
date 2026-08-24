//! Recovery action 的领取与 fencing。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(crate) async fn claim_action_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> Result<Option<RecoveryActionClaim>, auction_application::RepositoryError> {
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let Some(row) = sqlx::query(
        r#"SELECT action.id,
                  action.recovery_id,
                  action.kind,
                  action.execution_state,
                  action.delivery_id,
                  action.prepare_command_id,
                  action.prepare_request_digest,
                  action.activate_command_id,
                  action.activate_request_digest,
                  action.state_version,
                  action.claim_fencing,
                  action.retry_count,
                  action.source_mail_id,
                  action.expected_credits,
                  action.expected_cod,
                  action.expected_attachment_ids,
                  action.created_at,
                  recovery.world_id,
                  recovery.winner_user_id AS buyer_user_id,
                  recovery.payment_obligation_id,
                  source_mail.external_mail_id AS source_external_mail_id,
                  recipient.game_player_id AS recipient_game_player_id,
                  COALESCE(recipient.status = 'active', false) AS recipient_eligible,
                  COALESCE(recipient.state_version, 0) AS recipient_state_version
           FROM public.settlement_recovery_actions AS action
           INNER JOIN public.settlement_recoveries AS recovery
               ON recovery.id = action.recovery_id
           LEFT JOIN public.inbound_mail AS source_mail
               ON source_mail.id = action.source_mail_id
           LEFT JOIN public.users AS recipient
               ON recipient.id = recovery.winner_user_id
           WHERE action.kind IN (
                     'payment_reminder',
                     'reclaim_equipment_take',
                     'cancel_carrier_take',
                     'recover_equipment_take',
                     'buyer_blacklist'
                 )
             AND action.execution_state IN ('pending', 'prepared', 'enqueued')
             AND recovery.stage <> 'completed'
             AND recovery.blocking_code IS NULL
             AND (
                 action.kind <> 'buyer_blacklist'
                 OR EXISTS (
                     SELECT 1
                     FROM public.settlement_recovery_actions AS prerequisite
                     WHERE prerequisite.recovery_id = action.recovery_id
                       AND prerequisite.ordinal = 1
                       AND prerequisite.kind IN ('reclaim_equipment_take', 'cancel_carrier_take')
                       AND prerequisite.execution_state = 'satisfied'
                 )
             )
             AND (action.claim_token IS NULL OR action.claim_expires_at <= $1)
             AND (action.next_attempt_at IS NULL OR action.next_attempt_at <= $1)
           ORDER BY COALESCE(action.next_attempt_at, action.created_at),
                    action.recovery_id,
                    action.ordinal,
                    action.id
           LIMIT 1
           FOR UPDATE OF action SKIP LOCKED"#,
    )
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    else {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(None);
    };

    let action_id: Uuid = row.try_get("id").map_err(map_storage_error)?;
    let recovery_id: Uuid = row.try_get("recovery_id").map_err(map_storage_error)?;
    let kind: String = row.try_get("kind").map_err(map_storage_error)?;
    let execution_state: String = row.try_get("execution_state").map_err(map_storage_error)?;
    let delivery_id: Option<Uuid> = row.try_get("delivery_id").map_err(map_storage_error)?;
    let prepare_command_id: Option<Uuid> =
        row.try_get("prepare_command_id").map_err(map_storage_error)?;
    let prepare_request_digest: Option<Vec<u8>> =
        row.try_get("prepare_request_digest").map_err(map_storage_error)?;
    let activate_command_id: Option<Uuid> =
        row.try_get("activate_command_id").map_err(map_storage_error)?;
    let activate_request_digest: Option<Vec<u8>> =
        row.try_get("activate_request_digest").map_err(map_storage_error)?;
    let state_version: i64 = row.try_get("state_version").map_err(map_storage_error)?;
    let recovery_world_id: String = row.try_get("world_id").map_err(map_storage_error)?;
    let buyer_user_id: Uuid = row.try_get("buyer_user_id").map_err(map_storage_error)?;
    let payment_obligation_id: Uuid =
        row.try_get("payment_obligation_id").map_err(map_storage_error)?;
    let source_mail_id: Option<Uuid> = row.try_get("source_mail_id").map_err(map_storage_error)?;
    let source_external_mail_id: Option<String> =
        row.try_get("source_external_mail_id").map_err(map_storage_error)?;
    let expected_credits: i64 = row.try_get("expected_credits").map_err(map_storage_error)?;
    let expected_cod: i64 = row.try_get("expected_cod").map_err(map_storage_error)?;
    let expected_attachment_ids: Vec<String> =
        row.try_get("expected_attachment_ids").map_err(map_storage_error)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_storage_error)?;
    if action_id.is_nil()
        || recovery_id.is_nil()
        || buyer_user_id.is_nil()
        || !matches!(
            kind.as_str(),
            "payment_reminder"
                | "reclaim_equipment_take"
                | "cancel_carrier_take"
                | "recover_equipment_take"
                | "buyer_blacklist"
        )
        || !matches!(execution_state.as_str(), "pending" | "prepared" | "enqueued")
        || created_at > now
        || (execution_state == "enqueued" && delivery_id.is_none_or(|value| value.is_nil()))
        || (execution_state == "pending"
            && (delivery_id.is_some()
                || prepare_command_id.is_some()
                || prepare_request_digest.is_some()
                || activate_command_id.is_some()
                || activate_request_digest.is_some()))
        || (execution_state == "prepared"
            && (delivery_id.is_none_or(|value| value.is_nil())
                || prepare_command_id.is_none_or(|value| value.is_nil())
                || prepare_request_digest.as_deref().is_none_or(|value| value.len() != 32)
                || activate_command_id.is_some()
                || activate_request_digest.is_some()))
        || (execution_state == "enqueued"
            && (prepare_command_id.is_none_or(|value| value.is_nil())
                || prepare_request_digest.as_deref().is_none_or(|value| value.len() != 32)
                || activate_command_id.is_none_or(|value| value.is_nil())
                || activate_request_digest.as_deref().is_none_or(|value| value.len() != 32)))
        || !matches!(recovery_world_id.as_str(), "main" | "isekai")
        || expected_credits < 0
        || expected_cod < 0
        || expected_attachment_ids.len() > 1000
        || expected_attachment_ids.iter().any(|value| !valid_recovery_text(value, 128))
        || expected_attachment_ids.windows(2).any(|values| values[0] >= values[1])
        || (kind == "payment_reminder"
            && (source_mail_id.is_some()
                || source_external_mail_id.is_some()
                || expected_credits != 0
                || expected_cod != 0
                || !expected_attachment_ids.is_empty()))
        || (matches!(
            kind.as_str(),
            "reclaim_equipment_take" | "cancel_carrier_take" | "recover_equipment_take"
        ) && (source_mail_id.is_none()
            || source_external_mail_id
                .as_deref()
                .is_none_or(|value| !valid_external_reference(value))))
        || (kind == "buyer_blacklist"
            && (source_mail_id.is_some()
                || source_external_mail_id.is_some()
                || expected_credits != 0
                || expected_cod != 0
                || !expected_attachment_ids.is_empty()))
    {
        return Err(repository_error("recovery action 持久化事实无效"));
    }
    let world_id = match kind.as_str() {
        "payment_reminder" => "main".to_owned(),
        "cancel_carrier_take" => {
            if recovery_world_id != "isekai" {
                return Err(repository_error("recovery carrier action 世界不匹配"));
            }
            "main".to_owned()
        }
        "reclaim_equipment_take" => {
            if recovery_world_id != "main" {
                return Err(repository_error("recovery reclaim action 世界不匹配"));
            }
            "main".to_owned()
        }
        "recover_equipment_take" => {
            if recovery_world_id != "isekai" {
                return Err(repository_error("recovery equipment action 世界不匹配"));
            }
            "isekai".to_owned()
        }
        "buyer_blacklist" => recovery_world_id.clone(),
        _ => return Err(repository_error("recovery action 世界无法确定")),
    };

    let previous_fencing: i64 = row.try_get("claim_fencing").map_err(map_storage_error)?;
    let fencing_version = previous_fencing
        .checked_add(1)
        .ok_or_else(|| repository_error("recovery action fencing 版本溢出"))?;
    if fencing_version <= 0 {
        return Err(repository_error("recovery action fencing 版本无效"));
    }
    let previous_retry_count: i32 = row.try_get("retry_count").map_err(map_storage_error)?;
    let retry_count = previous_retry_count
        .checked_add(1)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| repository_error("recovery action 重试次数溢出"))?;
    let retry_count_db = i32::try_from(retry_count)
        .map_err(|_| repository_error("recovery action 重试次数超出数据库范围"))?;
    let claim_token = Uuid::new_v4();
    let updated = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET claim_token = $1,
               claimed_at = $2,
               claim_expires_at = $3,
               claim_fencing = $4,
               retry_count = $5,
               next_attempt_at = NULL,
               updated_at = $2
           WHERE id = $6
             AND execution_state = $7
             AND state_version > 0
             AND claim_fencing = $8
             AND (claim_token IS NULL OR claim_expires_at <= $2)
             AND (next_attempt_at IS NULL OR next_attempt_at <= $2)"#,
    )
    .bind(claim_token)
    .bind(now)
    .bind(lease_until)
    .bind(fencing_version)
    .bind(retry_count_db)
    .bind(action_id)
    .bind(&execution_state)
    .bind(previous_fencing)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error("recovery action 领取 fence 已失效"));
    }
    let recipient: Option<String> =
        row.try_get("recipient_game_player_id").map_err(map_storage_error)?;
    let recipient_eligible: bool = row.try_get("recipient_eligible").map_err(map_storage_error)?;
    let recipient_state_version: i64 =
        row.try_get("recipient_state_version").map_err(map_storage_error)?;
    if (recipient.is_some() && recipient_state_version <= 0) || recipient_state_version < 0 {
        return Err(repository_error("recovery 收件人 Users 状态版本无效"));
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(Some(RecoveryActionClaim {
        action_id: auction_domain::SettlementId::from_uuid(action_id),
        recovery_id: auction_domain::SettlementId::from_uuid(recovery_id),
        kind,
        world_id,
        buyer_user_id,
        recipient,
        recipient_eligible,
        recipient_state_version,
        payment_reference: payment_obligation_id.to_string(),
        source_external_mail_id,
        expected_credits,
        expected_cod,
        expected_attachment_ids,
        execution_state,
        delivery_id,
        prepare_command_id: prepare_command_id.map(CommandId::from_uuid),
        prepare_request_hash: request_hash_from_optional(prepare_request_digest)?,
        activate_command_id: activate_command_id.map(CommandId::from_uuid),
        activate_request_hash: request_hash_from_optional(activate_request_digest)?,
        claim_token,
        state_version,
        lease_until,
        fencing_version,
        retry_count,
        worker_id: worker_id.to_owned(),
        claimed_at: now,
        created_at,
    }))
}
