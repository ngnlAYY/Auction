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
        "SELECT status, external_reference, failure_code, result_metadata,
                command_id, request_digest, origin_kind, origin_id, recipient, payload,
                activation_command_id, activation_request_digest,
                public.delivery_has_frozen_sent_proof(id,result_metadata,external_reference) AS frozen_sent_proven
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
        "succeeded"
            if claim.kind == "payment_reminder" && !valid_reminder_delivery(claim, &row)? =>
        {
            manual_outcome("recovery_delivery_conflict")
        }
        "succeeded" => succeeded_outcome(
            claim,
            delivery_id,
            observed_at,
            external_reference,
            &result_metadata,
            row.try_get("frozen_sent_proven").map_err(map_storage_error)?,
        )?,
        "failed" if failure_code.as_deref().is_some_and(valid_failure_code) => (
            RecoveryActionOutcome::Failed { failure_code: "recovery_delivery_failed".to_owned() },
            RecoveryTerminalState::Failed { failure_code: "recovery_delivery_failed".to_owned() },
        ),
        "manual_reconcile" if failure_code.as_deref().is_some_and(valid_failure_code) => {
            manual_outcome("recovery_outcome_unknown")
        }
        // 缺少合法失败码的 failed/manual_reconcile 与未知状态都转人工核对。
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
    frozen_sent_proven: bool,
) -> Result<(RecoveryActionOutcome, RecoveryTerminalState), RecoveryActionTransactionError> {
    let (operation, world_id, proof_kind, expected_reference, proven) = match claim.kind.as_str() {
        "payment_reminder" => (
            "send",
            "main",
            "delivery_recovery_reminder_sent",
            None,
            result_metadata.get("external_reference_proven").and_then(serde_json::Value::as_bool)
                == Some(true)
                && result_metadata.get("operation").and_then(serde_json::Value::as_str)
                    == Some("send")
                && result_metadata.get("template").and_then(serde_json::Value::as_str)
                    == Some("recovery_payment_reminder")
                && ((result_metadata.get("sent_verified").and_then(serde_json::Value::as_bool)
                    == Some(true)
                    && result_metadata.get("proof").and_then(serde_json::Value::as_str)
                        == Some("sent_exact_payload_v1"))
                    || frozen_sent_proven),
        ),
        "reclaim_equipment_take" | "cancel_carrier_take" | "recover_equipment_take" => {
            let Some(expected_reference) = claim.source_external_mail_id.as_deref() else {
                return Ok(manual_outcome("recovery_delivery_conflict"));
            };
            (
                "take",
                claim.world_id.as_str(),
                "delivery_recovery_take_succeeded",
                Some(expected_reference),
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
        || expected_reference.is_some_and(|expected| reference != expected)
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
fn valid_reminder_delivery(
    claim: &RecoveryActionClaim,
    row: &sqlx::postgres::PgRow,
) -> Result<bool, RecoveryActionTransactionError> {
    let command_id: Uuid = row.try_get("command_id").map_err(map_storage_error)?;
    let request_digest: Vec<u8> = row.try_get("request_digest").map_err(map_storage_error)?;
    let origin_kind: String = row.try_get("origin_kind").map_err(map_storage_error)?;
    let origin_id: String = row.try_get("origin_id").map_err(map_storage_error)?;
    let recipient: String = row.try_get("recipient").map_err(map_storage_error)?;
    let payload: serde_json::Value = row.try_get("payload").map_err(map_storage_error)?;
    let activation_command_id: Option<Uuid> =
        row.try_get("activation_command_id").map_err(map_storage_error)?;
    let activation_digest: Option<Vec<u8>> =
        row.try_get("activation_request_digest").map_err(map_storage_error)?;
    // 付款 UUID 绑定冻结请求，远端 Sent MID 绑定成功回执；两种身份不能互相比较。
    // 使用 delivery 冻结收件人重算摘要，避免后来的用户名修改改变已发送事实。
    let expected_hash = RequestHash::from_request(
        "recovery_payment_reminder",
        claim.action_id.as_uuid(),
        &ReminderRequest {
            recovery_id: claim.recovery_id.as_uuid(),
            action_id: claim.action_id.as_uuid(),
            recipient: &recipient,
            payment_reference: &claim.payment_reference,
        },
    )
    .map_err(|_| repository_error("recovery 提醒请求摘要编码失败"))?;
    Ok(claim.prepare_command_id.map(CommandId::as_uuid) == Some(command_id)
        && claim.prepare_request_hash == Some(expected_hash)
        && request_digest == expected_hash.as_bytes()
        && activation_command_id.is_some()
        && activation_command_id == claim.activate_command_id.map(CommandId::as_uuid)
        && claim
            .activate_request_hash
            .is_some_and(|hash| activation_digest.as_deref() == Some(hash.as_bytes().as_slice()))
        && origin_kind == "settlement_recovery_action"
        && origin_id == format!("recovery:{}", claim.recovery_id)
        && payload
            == serde_json::json!({
                "schema_version": 1,
                "operation": "send",
                "world": "main",
                "external_mail_id": claim.payment_reference,
                "template": "recovery_payment_reminder",
                "result_code": "payment_overdue",
                "user_key_user_id": null,
                "credits": 0,
                "cod": 0,
                "attachment_ids": [],
                "body": null,
            }))
}

/// 字段顺序与 Worker 冻结的 recovery_payment_reminder 请求契约一致。
#[cfg(feature = "postgres")]
#[derive(Serialize)]
struct ReminderRequest<'a> {
    recovery_id: Uuid,
    action_id: Uuid,
    recipient: &'a str,
    payment_reference: &'a str,
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
