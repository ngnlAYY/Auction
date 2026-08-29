//! Recovery action 外部 delivery 结果读取、证明校验和终态收口。

#[cfg(feature = "postgres")]
use super::super::resolution::{RecoveryTerminalState, complete_action_in_transaction};
#[cfg(feature = "postgres")]
use super::*;
#[cfg(feature = "postgres")]
use serde::Serialize;

#[cfg(feature = "postgres")]
pub(crate) async fn observe_action_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    observed_at: DateTime<Utc>,
) -> Result<RecoveryActionOutcome, auction_application::RepositoryError> {
    let delivery_id =
        claim.delivery_id.ok_or_else(|| repository_error("recovery action 缺少 delivery 标识"))?;
    // 审计 ID 必须跨 Serializable 重试保持稳定。
    let audit_id = Uuid::new_v4();
    let claim_snapshot = claim.clone();
    run_serializable_action_transaction(pool, move |transaction| {
        let claim = claim_snapshot.clone();
        Box::pin(async move {
            observe_action_in_transaction(transaction, &claim, delivery_id, observed_at, audit_id)
                .await
        })
    })
    .await
}

#[cfg(feature = "postgres")]
async fn observe_action_in_transaction(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    claim: &RecoveryActionClaim,
    delivery_id: Uuid,
    observed_at: DateTime<Utc>,
    audit_id: Uuid,
) -> Result<RecoveryActionOutcome, RecoveryActionTransactionError> {
    let row = sqlx::query(
        "SELECT status, external_reference, failure_code, result_metadata
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(RecoveryActionTransactionError::from)?
    .ok_or_else(|| repository_error("recovery action delivery 不存在"))?;
    let status: String = row.try_get("status").map_err(map_storage_error)?;
    let external_reference: Option<String> =
        row.try_get("external_reference").map_err(map_storage_error)?;
    let failure_code: Option<String> = row.try_get("failure_code").map_err(map_storage_error)?;
    let result_metadata: serde_json::Value =
        row.try_get("result_metadata").map_err(map_storage_error)?;

    let (outcome, terminal) = match status.as_str() {
        "pending" | "claimed" | "sending" => return Ok(RecoveryActionOutcome::Retry),
        "prepared" => manual_outcome("recovery_delivery_conflict"),
        "succeeded" => succeeded_outcome(
            claim,
            delivery_id,
            observed_at,
            external_reference,
            &result_metadata,
        )?,
        "failed" if failure_code.as_deref().is_some_and(valid_failure_code) => (
            RecoveryActionOutcome::Failed { failure_code: "recovery_delivery_failed".to_owned() },
            RecoveryTerminalState::Failed { failure_code: "recovery_delivery_failed".to_owned() },
        ),
        "manual_reconcile" if failure_code.as_deref().is_some_and(valid_failure_code) => {
            manual_outcome("recovery_outcome_unknown")
        }
        "failed" | "manual_reconcile" => manual_outcome("recovery_delivery_conflict"),
        _ => manual_outcome("recovery_delivery_conflict"),
    };

    complete_action_in_transaction(transaction, claim, &terminal, audit_id).await?;
    Ok(outcome)
}

#[cfg(feature = "postgres")]
fn succeeded_outcome(
    claim: &RecoveryActionClaim,
    delivery_id: Uuid,
    observed_at: DateTime<Utc>,
    external_reference: Option<String>,
    result_metadata: &serde_json::Value,
) -> Result<(RecoveryActionOutcome, RecoveryTerminalState), RecoveryActionTransactionError> {
    let (operation, world_id, proof_kind, expected_reference, proven) = match claim.kind.as_str() {
        "payment_reminder" => (
            "send",
            "main",
            "delivery_recovery_reminder_sent",
            claim.payment_reference.as_str(),
            result_metadata.get("external_reference_proven").and_then(serde_json::Value::as_bool)
                == Some(true)
                && result_metadata.get("operation").and_then(serde_json::Value::as_str)
                    == Some("send")
                && result_metadata.get("template").and_then(serde_json::Value::as_str)
                    == Some("recovery_payment_reminder"),
        ),
        "reclaim_equipment_take" | "cancel_carrier_take" | "recover_equipment_take" => {
            let Some(expected_reference) = claim.source_external_mail_id.as_deref() else {
                return Ok(manual_outcome("recovery_delivery_conflict"));
            };
            (
                "take",
                claim.world_id.as_str(),
                "delivery_recovery_take_succeeded",
                expected_reference,
                result_metadata.get("operation").and_then(serde_json::Value::as_str)
                    == Some("take")
                    && result_metadata.get("external_mail_id").and_then(serde_json::Value::as_str)
                        == Some(expected_reference)
                    && result_metadata.get("proof").and_then(serde_json::Value::as_str)
                        == Some("action_removed_or_success_page")
                    && result_metadata.get("expected_credits").and_then(serde_json::Value::as_i64)
                        == Some(claim.expected_credits)
                    && result_metadata.get("expected_cod").and_then(serde_json::Value::as_i64)
                        == Some(claim.expected_cod)
                    && result_metadata
                        .get("expected_attachment_ids")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|values| {
                            values.len() == claim.expected_attachment_ids.len()
                                && values
                                    .iter()
                                    .zip(&claim.expected_attachment_ids)
                                    .all(|(value, expected)| value.as_str() == Some(expected))
                        }),
            )
        }
        _ => return Ok(manual_outcome("recovery_delivery_conflict")),
    };
    let Some(external_reference) =
        external_reference.filter(|value| valid_external_reference(value))
    else {
        return Ok(manual_outcome("recovery_outcome_unknown"));
    };
    let Some(reference) = recovery_reference_suffix(&external_reference, world_id, operation)
    else {
        return Ok(manual_outcome("recovery_outcome_unknown"));
    };
    if !proven
        || reference != expected_reference
        || observed_at < claim.claimed_at
        || observed_at > claim.lease_until
    {
        return Ok(manual_outcome("recovery_outcome_unknown"));
    }
    let proof_digest = recovery_delivery_proof_digest(
        claim,
        delivery_id,
        proof_kind,
        world_id,
        operation,
        &external_reference,
    )?;
    let proof_reference = reference.to_owned();
    Ok((
        RecoveryActionOutcome::Satisfied {
            proof_kind: proof_kind.to_owned(),
            proof_reference: proof_reference.clone(),
        },
        RecoveryTerminalState::Satisfied {
            proof_kind: proof_kind.to_owned(),
            proof_reference,
            proof_digest,
            proof_observed_at: observed_at,
        },
    ))
}

#[cfg(feature = "postgres")]
fn manual_outcome(failure_code: &str) -> (RecoveryActionOutcome, RecoveryTerminalState) {
    (
        RecoveryActionOutcome::ManualReconcile { failure_code: failure_code.to_owned() },
        RecoveryTerminalState::ManualReconcile { failure_code: failure_code.to_owned() },
    )
}

#[cfg(feature = "postgres")]
fn recovery_reference_suffix<'a>(
    external_reference: &'a str,
    world_id: &str,
    operation: &str,
) -> Option<&'a str> {
    let prefix = format!("mooglemail:{world_id}:{operation}:");
    external_reference.strip_prefix(&prefix).filter(|value| valid_external_reference(value))
}

#[cfg(feature = "postgres")]
#[derive(Serialize)]
struct RecoveryDeliveryProofPayload<'a> {
    schema_version: u8,
    kind: &'a str,
    delivery_id: String,
    status: &'static str,
    external_reference: &'a str,
    world: &'a str,
    operation: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_external_mail_id: Option<&'a str>,
    expected_credits: i64,
    expected_cod: i64,
    expected_attachment_ids: &'a [String],
}

#[cfg(feature = "postgres")]
fn recovery_delivery_proof_digest(
    claim: &RecoveryActionClaim,
    delivery_id: Uuid,
    proof_kind: &str,
    world_id: &str,
    operation: &str,
    external_reference: &str,
) -> Result<[u8; 32], RecoveryActionTransactionError> {
    let payload = RecoveryDeliveryProofPayload {
        schema_version: 1,
        kind: proof_kind,
        delivery_id: delivery_id.to_string(),
        status: "succeeded",
        external_reference,
        world: world_id,
        operation,
        source_external_mail_id: (operation == "take")
            .then_some(claim.source_external_mail_id.as_deref())
            .flatten(),
        expected_credits: claim.expected_credits,
        expected_cod: claim.expected_cod,
        expected_attachment_ids: &claim.expected_attachment_ids,
    };
    RequestHash::from_legacy_go_json(&payload)
        .map(RequestHash::as_bytes)
        .map_err(|_| repository_error("recovery delivery proof 摘要编码失败").into())
}
