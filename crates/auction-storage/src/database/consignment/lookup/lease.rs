//! 发送前复核与不改变网络硬截止的围栏续租。

use auction_application::consignment::{
    ListingLookupClaim, LookupDispatch, LookupFence, LookupLeaseUpdate,
};
use auction_time::Timestamp;
use sqlx::PgConnection;

use super::admission::{db_now, lock_pause, paused_at};
use super::control::lock_control;
use super::protocol::LEASE_GRACE;
use super::work::{LookupFenceCheck, check_lookup_fence};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::eligibility::predicates::listing_sql;

pub(super) async fn confirm_dispatch(
    conn: &mut PgConnection,
    claim: &ListingLookupClaim,
    worker_paused: bool,
) -> Result<LookupDispatch, WriteError> {
    let control = lock_control(conn).await?;
    if !control.admitting || control.enabled_epoch.cast_unsigned() != claim.service_epoch() {
        return Ok(LookupDispatch::Lost);
    }
    lock_pause(conn).await?;
    let fence = claim.fence();
    if !matches!(check_lookup_fence(conn, fence).await?, LookupFenceCheck::Matched(_)) {
        return Ok(LookupDispatch::Lost);
    }
    // 不变量 2、5、6：和领取一样只读资格；硬截止以持久事实和原领取的较早者为准。
    let at = db_now(conn).await?;
    let deadline: Option<Timestamp> = sqlx::query_scalar(concat!(
        "SELECT LEAST(w.network_deadline, $3::timestamptz, l.expires_at, w.lease_until, c.permit_until)
         FROM public.consignment_lookup_work w JOIN public.consignment_listings l ON l.id = w.listing_id",
        listing_sql!(eligibility_joins),
        "WHERE w.id = $1 AND l.state = 'resolving' AND ", listing_sql!(effective_active, "$2"),
        " AND LEAST(w.network_deadline, $3) > $2 AND w.lease_until > $2 AND c.permit_until > $2",
    ))
    .bind(fence.work_id())
    .bind(at)
    .bind(claim.deadline())
    .fetch_optional(&mut *conn)
    .await?;
    let Some(deadline) = deadline else {
        return Ok(LookupDispatch::Lost);
    };
    let paused = worker_paused || paused_at(conn, at).await?;
    Ok(if db_now(conn).await? >= deadline {
        LookupDispatch::Lost
    } else if paused {
        LookupDispatch::Paused
    } else {
        LookupDispatch::Allowed { deadline }
    })
}

pub(super) async fn renew(
    conn: &mut PgConnection,
    fence: LookupFence,
) -> Result<LookupLeaseUpdate<Timestamp>, WriteError> {
    let control = lock_control(conn).await?;
    if !control.admitting
        || !matches!(check_lookup_fence(conn, fence).await?, LookupFenceCheck::Matched(_))
    {
        return Ok(LookupLeaseUpdate::Lost);
    }
    // 不变量 2、5、6：重核两个租期的数据库时间与准入，更新只使用原工作持久化的
    // 硬截止。即使 Worker 没在截止前再发网，也可在余量内完成本地提交。
    let updated = sqlx::query_scalar(concat!(
        "WITH db_clock AS MATERIALIZED (SELECT clock_timestamp() AS at),
         eligible AS (
             SELECT at FROM public.consignment_listings l CROSS JOIN db_clock",
        listing_sql!(eligibility_joins),
        "WHERE l.id = $2 AND l.state = 'resolving' AND ",
        listing_sql!(effective_active, "at"),
        " AND c.permit_until > at
         ), renewed AS (
             UPDATE public.consignment_lookup_work w
             SET lease_until = w.network_deadline + make_interval(secs => $5), updated_at = at
             FROM eligible WHERE w.id = $1 AND w.fence = $3 AND w.token = $4 AND w.lease_until > at
               AND w.network_deadline + make_interval(secs => $5) > at
             RETURNING w.lease_until
         )
         UPDATE public.consignment_lookup_control c SET permit_until = renewed.lease_until
         FROM renewed WHERE c.id = 1
         RETURNING c.permit_until",
    ))
    .bind(fence.work_id())
    .bind(fence.listing_id())
    .bind(fence.fence())
    .bind(fence.token())
    .bind(LEASE_GRACE.as_secs_f64())
    .fetch_optional(conn)
    .await?;
    Ok(updated.map_or(LookupLeaseUpdate::Lost, LookupLeaseUpdate::Applied))
}
