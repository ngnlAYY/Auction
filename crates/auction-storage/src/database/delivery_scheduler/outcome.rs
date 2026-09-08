//! delivery 成功、失败和人工复核终态写回。

use super::mapping::*;

use super::*;

pub(super) async fn succeed_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    result: &DeliverySendResult,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    // 所有主动寄送共享同一成功门，通知、告警与结算不能绕过真实 Sent 证明。
    if claim.payload.get("operation").and_then(serde_json::Value::as_str) == Some("send")
        && !verified_sent_result(claim, result)
    {
        return Err(repository_error_owned("MoogleMail send 成功结果缺少完整 Sent 核验"));
    }
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let updated = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = 'succeeded',
               lease_token = NULL,
               lease_expires_at = NULL,
               completed_at = $2,
               external_reference = $3,
               failure_code = NULL,
               result_metadata = $4,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'sending'
             AND lease_token = $5
             AND fencing_version = $6
             AND lease_expires_at > $2"#,
    )
    .bind(claim.delivery_id)
    .bind(completed_at)
    .bind(&result.external_reference)
    .bind(&result.result_metadata)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 成功写回时 lease 已失效"));
    }
    if claim.origin_kind == "settlement" {
        let operation = result
            .result_metadata
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| repository_error_owned("结算 delivery 成功结果缺少 operation"))?;
        match operation {
            "return" => {
                if result
                    .result_metadata
                    .get("external_mail_id")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
                    || result.result_metadata.get("proof").and_then(serde_json::Value::as_str)
                        != Some("action_removed_or_success_page")
                {
                    return Err(repository_error_owned("结算 return 成功结果缺少动作完成证明"));
                }
            }
            // Sent 证明寄送成立；后续领取/COD 支付仍由 fulfillment owner 独立对账。
            "send" if verified_sent_result(claim, result) => {}
            _ => return Err(repository_error_owned("结算 delivery operation 无效")),
        }
    }
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: completed_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: SUCCEED,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata: worker_state_metadata(
                "succeeded",
                claim.fencing_version,
                claim.attempt_count,
                None,
            ),
        },
    )
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}

fn verified_sent_result(claim: &DeliveryWorkClaim, result: &DeliverySendResult) -> bool {
    let Some(world @ ("main" | "isekai")) =
        claim.payload.get("world").and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    let prefix = format!("mooglemail:{world}:send:");
    let Some(mail_id) = result.external_reference.strip_prefix(&prefix) else {
        return false;
    };
    !mail_id.is_empty()
        && mail_id.len() <= 256
        && mail_id == mail_id.trim()
        && !mail_id.chars().any(char::is_control)
        && result.result_metadata.get("operation").and_then(serde_json::Value::as_str)
            == Some("send")
        && result
            .result_metadata
            .get("external_reference_proven")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && result.result_metadata.get("sent_verified").and_then(serde_json::Value::as_bool)
            == Some(true)
        && result.result_metadata.get("proof").and_then(serde_json::Value::as_str)
            == Some("sent_exact_payload_v1")
}

#[cfg(feature = "postgres")]
pub(super) async fn fail_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    failure_code: &str,
    outcome_unknown: bool,
    external_reference: Option<&str>,
    result_metadata: &serde_json::Value,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
    let status = if outcome_unknown { "manual_reconcile" } else { "failed" };
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    let updated = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = $2,
               lease_token = NULL,
               lease_expires_at = NULL,
               completed_at = $3,
               external_reference = $5,
               failure_code = $4,
               result_metadata = $6,
               updated_at = CURRENT_TIMESTAMP
           WHERE id = $1
             AND status = 'sending'
             AND lease_token = $7
             AND fencing_version = $8
             AND lease_expires_at > $3"#,
    )
    .bind(claim.delivery_id)
    .bind(status)
    .bind(completed_at)
    .bind(failure_code)
    .bind(external_reference)
    .bind(result_metadata)
    .bind(claim.lease_token)
    .bind(claim.fencing_version)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error_owned("delivery 失败写回时 lease 已失效"));
    }
    // delivery 只提交自己的传输终态。fulfillment owner 会在后续领取中读取该状态，
    // 以自身 claim/fencing 原子写入 failed/manual/satisfied 和唯一 owner audit。
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: completed_at,
            actor_type: "worker",
            actor_id: "delivery-worker",
            action: if outcome_unknown { OUTCOME_UNKNOWN } else { FAIL },
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "worker",
            metadata: worker_state_metadata(
                status,
                claim.fencing_version,
                claim.attempt_count,
                Some(failure_code),
            ),
        },
    )
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(())
}
