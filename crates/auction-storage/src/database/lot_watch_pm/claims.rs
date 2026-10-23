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
        || request.available_worlds.len() > 2
    {
        return Err(LotWatchError::Invalid);
    }
    if request.available_worlds.is_empty() {
        return Ok(None);
    }
    let available_worlds = request
        .available_worlds
        .iter()
        .copied()
        .map(auction_application::forum_world_name)
        .collect::<Vec<_>>();
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
    // 来源世界来自不可变的 lot→auction 绑定；先按真实凭据 scope 筛选，避免不可用世界占用名额。
    let row = sqlx::query("WITH candidate AS (
        SELECT q.notification_id FROM public.lot_watch_pm_deliveries q
        JOIN public.lot_watch_notifications n ON n.id=q.notification_id AND n.channel='pm'
        JOIN public.lots lot ON lot.id=n.lot_id
        JOIN public.auctions auction ON auction.id=lot.auction_id
        WHERE (($1 AND q.status='manual_reconcile' AND q.reconcile_requested_at IS NOT NULL)
            OR (NOT $1 AND q.status IN ('pending','claimed') AND q.sending_at IS NULL))
        AND auction.world_id=ANY($5::text[])
        AND (q.lease_expires_at IS NULL OR q.lease_expires_at <= clock_timestamp())
        ORDER BY COALESCE(q.claimed_at,q.created_at),q.created_at,q.notification_id
        LIMIT 1 FOR UPDATE OF q SKIP LOCKED
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
    ) SELECT q.notification_id,q.fencing_version,q.sender_forum_account_id,n.lot_id,n.payload,n.recipient,n.request_digest,n.command_id,auction.world_id
      FROM claimed q JOIN public.lot_watch_notifications n ON n.id=q.notification_id AND n.channel='pm'
      JOIN public.lots lot ON lot.id=n.lot_id
      JOIN public.auctions auction ON auction.id=lot.auction_id")
        .bind(request.reconcile).bind(token).bind(request.lease_seconds).bind(&request.worker_id)
        .bind(&available_worlds)
        .fetch_optional(&mut *tx).await.map_err(sql_error)?;
    let result = row
        .as_ref()
        .map(|row| -> Result<LotWatchPmClaim, LotWatchError> {
            Ok(LotWatchPmClaim {
                message: message(row)?,
                lot_id: row.try_get("lot_id").map_err(sql_error)?,
                world: match row.try_get::<&str, _>("world_id").map_err(sql_error)? {
                    "main" => auction_domain::SettlementWorld::Main,
                    "isekai" => auction_domain::SettlementWorld::Isekai,
                    _ => return Err(LotWatchError::Unavailable),
                },
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
