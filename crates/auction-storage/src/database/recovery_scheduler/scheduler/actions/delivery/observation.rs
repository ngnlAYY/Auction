//! Recovery action 外部 delivery 结果读取、证明校验和终态收口。

#[cfg(feature = "postgres")]
use super::*;

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
