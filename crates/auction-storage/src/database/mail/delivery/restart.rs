//! 结算退件 Take 投递的“确定未外写”重启：由案件 owner 在锁内复核持久发送围栏与尝试历史后，把
//! 终态 failed 的 Take 投递重新排队。只接受结果元数据中 `write_started` 与 `outcome_unknown` 都是
//! 明确的 JSON 布尔 `false`（缺键、null 或错误类型都按可能已执行）、从未被巡检因结果未知重排过
//! （`sweep_retries`）且没有远端引用的投递；其余失败或未知一律不动，仍只认领取证明。

use serde_json::json;
use uuid::Uuid;

use crate::database::delivery::audit::{DeliveryAudit, RELEASE, append as append_delivery_audit};

/// 重启一项确定未外写的结算退件 Take 投递；没有改写任何行时返回 `false`。新 fence 让旧尝试的
/// 一切迟到结果失效，`not_started_restarts` 记录本次是第几次重启。调用方已持有案件、动作与投递
/// 行锁，并在同一事务推进案件。
pub(in crate::database) async fn restart_unstarted_take_in_transaction(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    delivery_id: Uuid,
    restarts: i64,
) -> Result<bool, sqlx::Error> {
    let restarted: Option<(chrono::DateTime<chrono::Utc>, i64)> = sqlx::query_as(
        "UPDATE public.deliveries
         SET status = 'pending', lease_token = NULL, lease_expires_at = NULL, claimed_at = NULL,
             sending_at = NULL, completed_at = NULL, failure_code = NULL,
             result_metadata = COALESCE(result_metadata, '{}'::jsonb)
                 || jsonb_build_object('not_started_restarts', $2::bigint,
                                       'not_started_restarted_at', clock_timestamp()),
             fencing_version = fencing_version + 1,
             updated_at = clock_timestamp()
         WHERE id = $1 AND status = 'failed'
           AND origin_kind = 'settlement_return_action' AND payload_operation = 'take'
           AND external_reference IS NULL
           AND COALESCE(result_metadata
                   @> '{\"write_started\": false, \"outcome_unknown\": false}'::jsonb, false)
           AND NOT (result_metadata ? 'sweep_retries')
         RETURNING updated_at, fencing_version",
    )
    .bind(delivery_id)
    .bind(restarts)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((occurred_at, fencing_version)) = restarted else {
        return Ok(false);
    };
    append_delivery_audit(
        transaction,
        DeliveryAudit {
            occurred_at,
            actor_type: "worker",
            actor_id: "settlement-returns",
            action: RELEASE,
            delivery_id,
            request_id: None,
            source: "worker",
            metadata: json!({
                "status": "pending",
                "not_started_restart": restarts,
                "fencing_version": fencing_version,
            }),
        },
    )
    .await?;
    Ok(true)
}
