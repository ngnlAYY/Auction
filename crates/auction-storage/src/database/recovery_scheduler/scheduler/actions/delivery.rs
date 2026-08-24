//! Recovery action 的投递准备、激活与结果观察。

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

#[cfg(feature = "postgres")]
pub(crate) async fn observe_action_postgres(
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
