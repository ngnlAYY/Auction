//! 过期sending逐条隔离，早锁持久资金账户；不能凭TTL释放占用。

use auction_time::Timestamp;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::outcome::OutcomeError;
use crate::database::delivery::audit::{
    DeliveryAudit, RECONCILE_EXPIRED_SENDING, append as append_delivery_audit,
};
use crate::database::delivery::scheduler::{EXPIRED_SENDING_RECONCILE_LIMIT, mapping};
use crate::database::settlement::funding;

pub(super) async fn reconcile(
    pool: &crate::database::TrackedPool<Postgres>,
    now: Timestamp,
    grace_micros: i64,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
) -> Result<(), auction_application::core::RepositoryError> {
    // 候选读取不持业务锁；每条短事务内重新锁定并核验有效期。
    // 寄售处置投递与寄售轮投递的过期由寄售专用链按原起写事实核对，不能被通用回收加 fence、
    // 改成人工对账（不变量 4：结果未知只转人工核对，绝不自动重发；起写许可只发一次）。
    // 寄售轮起写后的结果未知由寄售派发路径（已起写、无终局证据 → 未知派发）承担。
    let candidates = sqlx::query_scalar::<_, Uuid>(concat!(
        "SELECT id FROM public.deliveries AS deliveries WHERE status='sending'
         AND lease_expires_at+($2::bigint*interval '1 microsecond')<=$1
         AND ",
        delivery_generic_origin_guard!(),
        " AND ",
        delivery_not_consignment_round_guard!(),
        " ORDER BY lease_expires_at,id LIMIT $3"
    ))
    .bind(now)
    .bind(grace_micros)
    .bind(EXPIRED_SENDING_RECONCILE_LIMIT)
    .fetch_all(pool)
    .await
    .map_err(mapping::map_storage_error)?;
    for id in candidates {
        crate::database::retry_conflicts(
            pool,
            auction_observability::DatabaseTransactionOwner::DeliveryOutcome,
            crate::database::SERIALIZABLE_ATTEMPTS,
            OutcomeError::retryable,
            || expire_one(pool, id, now, grace_micros, notifications),
        )
        .await
        .map_err(|_| mapping::repository_error("过期delivery资金隔离未确定完成"))?;
    }
    Ok(())
}

async fn expire_one(
    pool: &crate::database::TrackedPool<Postgres>,
    id: Uuid,
    now: Timestamp,
    grace_micros: i64,
    notifications: &dyn auction_application::notification_metrics::NotificationRecorder,
) -> Result<(), OutcomeError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    let account = funding::lock_funded_delivery_account(&mut tx, id).await?;
    sqlx::query("SELECT id FROM public.deliveries WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    // 过期只改变本次结果与 CAS fence，不能重置自动维护的跨尝试重排预算。
    // 同一来源排除在锁内 UPDATE 再执行一次：候选扫描与逐条回收之间可能有新行或旧 Worker
    // 推进 fence（不变量 4，寄售 owner 的起写许可只发一次）。
    let row=sqlx::query(concat!(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         UPDATE public.deliveries AS deliveries SET status='manual_reconcile',lease_token=NULL,lease_expires_at=NULL,
         completed_at=$2,external_reference=NULL,failure_code='lease_expired_outcome_unknown',
         result_metadata='{\"outcome_unknown\":true,\"reason\":\"sending_lease_expired\"}'::jsonb
             || CASE WHEN result_metadata ? 'sweep_retries'
                 THEN jsonb_build_object('sweep_retries',result_metadata->'sweep_retries')
                 ELSE '{}'::jsonb END,
         fencing_version=fencing_version+1,updated_at=(SELECT at FROM sql_clock)
         WHERE id=$1 AND status='sending'
         AND lease_expires_at+($3::bigint*interval '1 microsecond')<=$2
         AND lease_expires_at+($3::bigint*interval '1 microsecond')<=(SELECT at FROM sql_clock)
         AND ",
        delivery_generic_origin_guard!(),
        " AND ",
        delivery_not_consignment_round_guard!(),
        " RETURNING command_id,fencing_version,attempt_count,payload,created_at,origin_kind"
    )).bind(id).bind(now).bind(grace_micros).fetch_optional(&mut *tx).await?;
    if let Some(row) = row {
        if let Some(account) = account {
            funding::finish_funded_delivery(
                &mut tx,
                &account,
                id,
                auction_application::settlement::FundingDeliveryOutcome::Unknown,
            )
            .await?;
        }
        let command_id: Uuid = row.try_get("command_id")?;
        // 过期 sending 的远端结果未知：按通知种类记一次失败与延迟，不改变人工对账语义。
        let payload: serde_json::Value = row.try_get("payload")?;
        let origin_kind: String = row.try_get("origin_kind")?;
        let created_at: Timestamp = row.try_get("created_at")?;
        if let Some(kind) =
            auction_application::notification_metrics::NotificationKind::from_delivery(
                &origin_kind,
                payload.get("operation").and_then(serde_json::Value::as_str),
                payload.get("template").and_then(serde_json::Value::as_str),
            )
        {
            notifications.observe_stage(
                kind,
                auction_application::notification_metrics::NotificationStage::Failed,
            );
            notifications.observe_failure(
                kind,
                auction_application::notification_metrics::NotificationFailure::LeaseExpiredOutcomeUnknown,
            );
            if let Ok(latency) = (now - created_at).to_std() {
                notifications.observe_delivery_latency(kind, latency);
            }
        }
        let request_id = command_id.to_string();
        append_delivery_audit(&mut tx,DeliveryAudit {
            occurred_at:now,actor_type:"worker",actor_id:"delivery-worker",action:RECONCILE_EXPIRED_SENDING,
            delivery_id:id,request_id:Some(&request_id),source:"worker",
            metadata:serde_json::json!({"status":"manual_reconcile","failure_code":"lease_expired_outcome_unknown",
                "fencing_version":row.try_get::<i64,_>("fencing_version")?,"attempt_count":row.try_get::<i32,_>("attempt_count")?}),
        }).await?;
    }
    tx.commit().await?;
    Ok(())
}
