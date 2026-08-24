//! Recovery action 的领取、关联、投递、完成、观察、释放与人工复核。
//!
//! 该文件只承载 recovery Worker 的单一职责，事务入口和外部副作用仍由父模块统一约束。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn claim_action_postgres(
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

#[cfg(feature = "postgres")]
pub(super) async fn link_action_postgres(
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

#[cfg(feature = "postgres")]
pub(super) async fn mark_action_enqueued_postgres(
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

#[cfg(feature = "postgres")]
pub(super) async fn mark_action_satisfied_postgres(
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
pub(super) async fn observe_action_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    observed_at: DateTime<Utc>,
) -> Result<RecoveryActionOutcome, auction_application::RepositoryError> {
    let delivery_id =
        claim.delivery_id.ok_or_else(|| repository_error("recovery action 缺少 delivery 标识"))?;
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let row = sqlx::query(
        "SELECT status, external_reference, failure_code, result_metadata
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("recovery action delivery 不存在"))?;
    let status: String = row.try_get("status").map_err(map_storage_error)?;
    let external_reference: Option<String> =
        row.try_get("external_reference").map_err(map_storage_error)?;
    let failure_code: Option<String> = row.try_get("failure_code").map_err(map_storage_error)?;
    let result_metadata: serde_json::Value =
        row.try_get("result_metadata").map_err(map_storage_error)?;

    let (outcome, execution_state, terminal_error, proof_kind, proof_reference, proof_digest) =
        match status.as_str() {
            "prepared" | "pending" | "claimed" | "sending" => {
                transaction.commit().await.map_err(map_storage_error)?;
                return Ok(RecoveryActionOutcome::Retry);
            }
            "succeeded" => {
                let proven = match claim.kind.as_str() {
                    "payment_reminder" => {
                        result_metadata
                            .get("external_reference_proven")
                            .and_then(serde_json::Value::as_bool)
                            == Some(true)
                            && result_metadata.get("operation").and_then(serde_json::Value::as_str)
                                == Some("send")
                            && result_metadata.get("template").and_then(serde_json::Value::as_str)
                                == Some("recovery_payment_reminder")
                    }
                    "reclaim_equipment_take" | "cancel_carrier_take" | "recover_equipment_take" => {
                        result_metadata.get("operation").and_then(serde_json::Value::as_str)
                            == Some("take")
                            && result_metadata
                                .get("external_mail_id")
                                .and_then(serde_json::Value::as_str)
                                == claim.source_external_mail_id.as_deref()
                            && result_metadata.get("proof").and_then(serde_json::Value::as_str)
                                == Some("action_removed_or_success_page")
                            && result_metadata
                                .get("expected_credits")
                                .and_then(serde_json::Value::as_i64)
                                == Some(claim.expected_credits)
                            && result_metadata
                                .get("expected_cod")
                                .and_then(serde_json::Value::as_i64)
                                == Some(claim.expected_cod)
                            && result_metadata
                                .get("expected_attachment_ids")
                                .and_then(serde_json::Value::as_array)
                                .is_some_and(|values| {
                                    values.len() == claim.expected_attachment_ids.len()
                                        && values.iter().zip(&claim.expected_attachment_ids).all(
                                            |(value, expected)| value.as_str() == Some(expected),
                                        )
                                })
                    }
                    _ => false,
                };
                let reference_matches = match claim.kind.as_str() {
                    "payment_reminder" => {
                        external_reference.as_deref().is_some_and(valid_external_reference)
                    }
                    "reclaim_equipment_take" | "cancel_carrier_take" | "recover_equipment_take" => {
                        claim.source_external_mail_id.as_deref().is_some_and(|source| {
                            let expected_reference =
                                format!("mooglemail:{}:take:{source}", claim.world_id);
                            external_reference.as_deref() == Some(expected_reference.as_str())
                        })
                    }
                    _ => false,
                };
                match external_reference.filter(|value| valid_external_reference(value)) {
                    Some(reference) if proven && reference_matches => {
                        let digest = recovery_proof_digest(claim.action_id.as_uuid(), &reference);
                        let (proof_kind, proof_reference) = if claim.kind == "payment_reminder" {
                            ("delivery_recovery_reminder_sent".to_owned(), reference.clone())
                        } else {
                            ("delivery_recovery_take_completed".to_owned(), reference.clone())
                        };
                        (
                            RecoveryActionOutcome::Satisfied {
                                proof_kind: proof_kind.clone(),
                                proof_reference: proof_reference.clone(),
                            },
                            "satisfied",
                            None,
                            Some(proof_kind),
                            Some(proof_reference),
                            Some(digest.to_vec()),
                        )
                    }
                    _ => (
                        RecoveryActionOutcome::ManualReconcile {
                            failure_code: if proven && !reference_matches {
                                "delivery_reference_mismatch".to_owned()
                            } else if proven {
                                "delivery_reference_missing".to_owned()
                            } else {
                                "delivery_reference_unproven".to_owned()
                            },
                        },
                        "manual_reconcile",
                        Some(if proven && !reference_matches {
                            "delivery_reference_mismatch".to_owned()
                        } else if proven {
                            "delivery_reference_missing".to_owned()
                        } else {
                            "delivery_reference_unproven".to_owned()
                        }),
                        None,
                        None,
                        None,
                    ),
                }
            }
            "failed" => {
                let code = failure_code
                    .filter(|value| valid_failure_code(value))
                    .unwrap_or_else(|| "delivery_failed".to_owned());
                (
                    RecoveryActionOutcome::Failed { failure_code: code.clone() },
                    "failed",
                    Some(code),
                    None,
                    None,
                    None,
                )
            }
            "manual_reconcile" => {
                let code = failure_code
                    .filter(|value| valid_failure_code(value))
                    .unwrap_or_else(|| "delivery_outcome_unknown".to_owned());
                (
                    RecoveryActionOutcome::ManualReconcile { failure_code: code.clone() },
                    "manual_reconcile",
                    Some(code),
                    None,
                    None,
                    None,
                )
            }
            _ => {
                transaction.commit().await.map_err(map_storage_error)?;
                return Err(repository_error("recovery action delivery 状态未知"));
            }
        };

    let updated = sqlx::query(
        r#"UPDATE public.settlement_recovery_actions
           SET execution_state = $1,
               last_error_code = $2,
               proof_kind = $3,
               proof_reference = $4,
               proof_digest = $5,
               proof_observed_at = $6,
               completed_at = $6,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $6
           WHERE id = $7
             AND recovery_id = $8
             AND execution_state = 'enqueued'
             AND state_version = $9
             AND claim_token = $10
             AND claim_fencing = $11"#,
    )
    .bind(execution_state)
    .bind(terminal_error)
    .bind(proof_kind)
    .bind(proof_reference)
    .bind(proof_digest)
    .bind(observed_at)
    .bind(claim.action_id.as_uuid())
    .bind(claim.recovery_id.as_uuid())
    .bind(claim.state_version)
    .bind(claim.claim_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error("recovery action 结果 fence 已失效"));
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(outcome)
}

#[cfg(feature = "postgres")]
pub(super) async fn release_action_postgres(
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
pub(super) async fn manual_action_postgres(
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
