//! Delivery 人工重试、精确发送证明与 Recovery 父子投影的原子衔接。

use crate::database::TrackedTransaction as Transaction;
use sqlx::{Postgres, Row};
use uuid::Uuid;

#[path = "delivery_retry/locks.rs"]
mod locks;
pub(crate) use locks::{lock_recovery_delivery_retry_actions, lock_recovery_delivery_retry_scope};

/// 调用方须先按统一序调用scope→中间业务锁→actions，再锁delivery、复核版本与人工授权。
/// 两阶段前缀和本函数必须在同一Serializable事务；这里只更新已预锁行并追加原命令/审计。
/// 这里只恢复失败投影，不生成成功证明、不重新准备资产动作，也不清除其它 action 的阻断。
pub(crate) async fn reset_after_delivery_retry(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT action.id, action.recovery_id, action.execution_state,
                action.last_error_code, recovery.blocking_code,
                recovery.manual_reconcile_delivery_id, recovery.stage, recovery.resolution
         FROM public.settlement_recovery_actions AS action
         INNER JOIN public.settlement_recoveries AS recovery ON recovery.id = action.recovery_id
         WHERE action.delivery_id = $1
         ORDER BY action.id",
    )
    .bind(delivery_id)
    .fetch_all(&mut **transaction)
    .await?;
    let Some(row) = rows.first() else { return Ok(true) };
    if rows.len() != 1
        || row.try_get::<String, _>("stage")? == "completed"
        || row.try_get::<Option<String>, _>("resolution")?.is_some()
    {
        return Ok(false);
    }
    let state: String = row.try_get("execution_state")?;
    let blocker: Option<String> = row.try_get("blocking_code")?;
    let manual_delivery: Option<Uuid> = row.try_get("manual_reconcile_delivery_id")?;
    if state == "enqueued" {
        // Worker 尚未投影 delivery 失败时，无需重写仍有效的 action claim。
        return Ok(blocker.is_none() && manual_delivery.is_none());
    }
    let expected_error = match state.as_str() {
        "failed" if manual_delivery.is_none() => "recovery_delivery_failed",
        "manual_reconcile" if manual_delivery == Some(delivery_id) => "recovery_outcome_unknown",
        _ => return Ok(false),
    };
    if blocker.as_deref() != Some(expected_error)
        || row.try_get::<Option<String>, _>("last_error_code")?.as_deref() != Some(expected_error)
    {
        return Ok(false);
    }
    let action_id: Uuid = row.try_get("id")?;
    let recovery_id: Uuid = row.try_get("recovery_id")?;
    let other_blocker: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.settlement_recovery_actions
             WHERE recovery_id = $1 AND id <> $2
               AND execution_state IN ('failed', 'manual_reconcile')
         )",
    )
    .bind(recovery_id)
    .bind(action_id)
    .fetch_one(&mut **transaction)
    .await?;
    if other_blocker {
        return Ok(false);
    }
    let reset = sqlx::query(
        "UPDATE public.settlement_recovery_actions
         SET execution_state = 'enqueued', last_error_code = NULL,
             claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL,
             claim_fencing = claim_fencing + 1, retry_count = 0,
             next_attempt_at = NULL, completed_at = NULL,
             state_version = state_version + 1, updated_at = clock_timestamp()
         WHERE id = $1 AND delivery_id = $2 AND execution_state = $3
           AND prepare_command_id IS NOT NULL AND octet_length(prepare_request_digest) = 32
           AND activate_command_id IS NOT NULL AND octet_length(activate_request_digest) = 32
           AND proof_kind IS NULL AND proof_reference IS NULL
           AND proof_digest IS NULL AND proof_observed_at IS NULL
           AND (claim_token IS NULL OR claim_expires_at <= clock_timestamp())",
    )
    .bind(action_id)
    .bind(delivery_id)
    .bind(state)
    .execute(&mut **transaction)
    .await?;
    if reset.rows_affected() != 1 {
        return Ok(false);
    }
    let projected = sqlx::query(
        "UPDATE public.settlement_recoveries
         SET blocking_code = NULL, manual_reconcile_delivery_id = NULL,
             state_version = state_version + 1, updated_at = clock_timestamp()
         WHERE id = $1 AND stage <> 'completed' AND resolution IS NULL
           AND blocking_code = $2",
    )
    .bind(recovery_id)
    .bind(expected_error)
    .execute(&mut **transaction)
    .await?;
    Ok(projected.rows_affected() == 1)
}

/// 调用方已按 recovery→credentials→actions→delivery 锁序取得相关行，并写入真实发送证明。
/// 只恢复由这封提醒的未知结果形成的阻断；不重排 delivery，不清除别的动作或人工业务阻断。
pub(crate) async fn resume_after_delivery_proof(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let resumed: Option<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE public.settlement_recovery_actions AS action
         SET execution_state='enqueued',last_error_code=NULL,
             claim_token=NULL,claimed_at=NULL,claim_expires_at=NULL,
             claim_fencing=action.claim_fencing+1,state_version=action.state_version+1,
             next_attempt_at=NULL,completed_at=NULL,updated_at=clock_timestamp()
         FROM public.settlement_recoveries AS recovery,public.deliveries AS delivery
         WHERE action.delivery_id=$1 AND delivery.id=$1
           AND recovery.id=action.recovery_id AND recovery.stage<>'completed'
           AND recovery.resolution IS NULL AND action.kind='payment_reminder'
           AND action.execution_state='manual_reconcile'
           AND action.last_error_code='recovery_outcome_unknown'
           AND action.proof_kind IS NULL AND action.proof_reference IS NULL
           AND action.proof_digest IS NULL AND action.proof_observed_at IS NULL
           AND delivery.origin_kind='settlement_recovery_action'
           AND delivery.origin_id='recovery:'||recovery.id::text
           AND delivery.command_id=action.prepare_command_id
           AND delivery.request_digest=action.prepare_request_digest
           AND delivery.activation_command_id=action.activate_command_id
           AND delivery.activation_request_digest=action.activate_request_digest
           AND delivery.payload->>'operation'='send'
           AND delivery.payload->>'template'='recovery_payment_reminder'
           AND public.delivery_has_frozen_sent_proof(
               delivery.id,delivery.result_metadata,delivery.external_reference)
         RETURNING action.id,action.recovery_id",
    )
    .bind(delivery_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some((action_id, recovery_id)) = resumed else { return Ok(None) };
    // 父 recovery 已锁定；其它动作的完成/失败也先锁此行，因此当前阻断投影不会穿插变化。
    sqlx::query(
        "WITH other_blocker AS (
             SELECT COALESCE(last_error_code,'recovery_delivery_conflict') AS code,
                    CASE WHEN execution_state='manual_reconcile' THEN delivery_id ELSE NULL END AS delivery_id
             FROM public.settlement_recovery_actions
             WHERE recovery_id=$1 AND id<>$2 AND execution_state IN ('failed','manual_reconcile')
             ORDER BY id LIMIT 1
         )
         UPDATE public.settlement_recoveries
         SET blocking_code=(SELECT code FROM other_blocker),
             manual_reconcile_delivery_id=(SELECT delivery_id FROM other_blocker),
             state_version=state_version+1,updated_at=clock_timestamp()
         WHERE id=$1 AND stage<>'completed' AND resolution IS NULL
           AND blocking_code='recovery_outcome_unknown' AND manual_reconcile_delivery_id=$3",
    )
    .bind(recovery_id)
    .bind(action_id)
    .bind(delivery_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO public.audit_records
         (id,occurred_at,actor_type,actor_id,action,entity_type,entity_id,result,source,metadata)
         VALUES($1,clock_timestamp(),'system','auction-recovery','auction.recovery.delivery_proven',
                'settlement_recovery_action',$2,'success','worker',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(action_id.to_string())
    .bind(serde_json::json!({
        "delivery_id": delivery_id,
        "recovery_id": recovery_id,
        "state": "enqueued",
        "delivery_requeued": false,
    }))
    .execute(&mut **transaction)
    .await?;
    Ok(Some(action_id))
}
