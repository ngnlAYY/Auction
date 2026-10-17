//! 有界持久领取；过期 sending 只能转原 sender 对账，不回发送队列。
use super::*;

pub(super) async fn claim(
    store: &DatabaseStore,
    request: LotWatchPmClaimRequest,
) -> Result<Option<LotWatchPmClaim>, LotWatchError> {
    if request.worker_id.is_empty()
        || request.worker_id.len() > 128
        || request.lease_seconds <= 0
        || request.lease_seconds > 86400
    {
        return Err(LotWatchError::Invalid);
    }
    let DatabaseStore::Postgres(store) = store;
    let mut tx = store.pool().begin().await.map_err(sql_error)?;
    // 每轮最多恢复一项；不把未知写入误当成可再次发送。
    sqlx::query("UPDATE public.lot_watch_pm_deliveries SET status='manual_reconcile', failure_code='outcome_unknown',
        lease_token=NULL,lease_expires_at=NULL,lease_kind=NULL,worker_id=NULL,
        fencing_version=fencing_version+1,reconcile_requested_at=clock_timestamp(),updated_at=clock_timestamp()
        WHERE notification_id IN (SELECT notification_id FROM public.lot_watch_pm_deliveries
        WHERE status='sending' AND lease_expires_at <= clock_timestamp() ORDER BY lease_expires_at LIMIT 1 FOR UPDATE SKIP LOCKED)")
        .execute(&mut *tx).await.map_err(sql_error)?;
    let token = Uuid::new_v4();
    // 持久领取时间把失败或超时的准备任务排到其他已等待任务之后；UUID 稳定打破并列。
    // release 不清空 claimed_at，重启也不能让最早创建的坏收件人独占每轮名额。
    let row = sqlx::query("WITH candidate AS (
        SELECT notification_id FROM public.lot_watch_pm_deliveries
        WHERE (($1 AND status='manual_reconcile' AND reconcile_requested_at IS NOT NULL)
            OR (NOT $1 AND status IN ('pending','claimed') AND sending_at IS NULL))
        AND (lease_expires_at IS NULL OR lease_expires_at <= clock_timestamp())
        ORDER BY COALESCE(claimed_at,created_at),created_at,notification_id LIMIT 1 FOR UPDATE SKIP LOCKED
    ), claimed AS (
        UPDATE public.lot_watch_pm_deliveries q SET
        status=CASE WHEN $1 THEN 'manual_reconcile' ELSE 'claimed' END,
        fencing_version=fencing_version+1, lease_token=$2,
        lease_expires_at=clock_timestamp()+make_interval(secs => $3::bigint::double precision),
        lease_kind=CASE WHEN $1 THEN 'reconcile' ELSE 'send' END,worker_id=$4,
        claimed_at=clock_timestamp(),updated_at=clock_timestamp(),
        attempt_count=attempt_count+CASE WHEN $1 THEN 0 ELSE 1 END,
        reconcile_attempt_count=reconcile_attempt_count+CASE WHEN $1 THEN 1 ELSE 0 END
        FROM candidate WHERE q.notification_id=candidate.notification_id
        RETURNING q.*
    ) SELECT q.notification_id,q.fencing_version,q.sender_forum_account_id,n.lot_id,n.payload,n.recipient,n.request_digest,n.command_id
      FROM claimed q JOIN public.lot_watch_notifications n ON n.id=q.notification_id AND n.channel='pm'")
        .bind(request.reconcile).bind(token).bind(request.lease_seconds).bind(&request.worker_id)
        .fetch_optional(&mut *tx).await.map_err(sql_error)?;
    let result = row
        .as_ref()
        .map(|row| -> Result<LotWatchPmClaim, LotWatchError> {
            Ok(LotWatchPmClaim {
                message: message(row)?,
                lot_id: row.try_get("lot_id").map_err(sql_error)?,
                digest: row.try_get("request_digest").map_err(sql_error)?,
                token,
                fence: row.try_get("fencing_version").map_err(sql_error)?,
                sender: row.try_get("sender_forum_account_id").map_err(sql_error)?,
                reconcile: request.reconcile,
            })
        })
        .transpose()?;
    tx.commit().await.map_err(sql_error)?;
    Ok(result)
}
