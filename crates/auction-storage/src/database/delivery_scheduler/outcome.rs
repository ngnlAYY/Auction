//! delivery 成功、失败和人工复核终态写回。

use super::mapping::*;

use super::*;

pub(super) async fn succeed_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &DeliveryWorkClaim,
    result: &DeliverySendResult,
    completed_at: DateTime<Utc>,
) -> Result<(), auction_application::RepositoryError> {
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
            // send 的 POST 响应最多只能证明请求已被提交。旧版还会读取 Sent
            // 明细和余额来核对远端事实；在该证据接入前必须保持 enqueued，
            // 不能因为 delivery 成功就伪造 satisfied。
            "send" => {
                if result
                    .result_metadata
                    .get("external_reference_proven")
                    .and_then(serde_json::Value::as_bool)
                    .is_none()
                {
                    return Err(repository_error_owned(
                        "结算 send 成功结果缺少 external_reference_proven",
                    ));
                }
            }
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
