//! Worker 生命周期唤醒提示：只读计算下一次可领取时刻，不加锁、不领取，也不改变调度事实。

use chrono::{DateTime, Utc};
use sqlx::Postgres;

use super::mapping::map_storage_error;
use crate::database::TrackedPool;

/// 与 `claim.rs` 的 Preview、Open 与 Final 到期口径一致，再与失败退避写入的 `lease_until`
/// 取较晚值。只返回严格晚于 `now` 的时刻；已过期仍未领取的轮次交给常规轮询，避免空转。
pub(super) async fn next_claimable_postgres(
    pool: &TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, auction_application::RepositoryError> {
    sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT MIN(GREATEST(candidate.due_at, candidate.lease_until))
         FROM (
             SELECT round.lease_until,
                    CASE
                        WHEN round.preview_completed_at IS NULL THEN round.preview_due_at
                        WHEN round.open_completed_at IS NULL THEN round.open_due_at
                        ELSE deadline.earliest_end_at
                            - ((auction.policy_snapshot ->> 'final_phase_lead_seconds')
                                ::double precision * interval '1 second')
                    END AS due_at
             FROM public.scheduled_rounds AS round
             INNER JOIN public.auctions AS auction ON auction.id = round.auction_id
             LEFT JOIN LATERAL (
                 SELECT MIN(lot.effective_end_at) AS earliest_end_at
                 FROM public.lots AS lot
                 WHERE lot.auction_id = round.auction_id AND lot.status = 'live'
                   AND round.open_completed_at IS NOT NULL AND auction.status = 'live'
             ) AS deadline ON true
             WHERE round.status = 'active' AND round.final_completed_at IS NULL
         ) AS candidate
         WHERE candidate.due_at IS NOT NULL
           AND GREATEST(candidate.due_at, candidate.lease_until) > $1",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(map_storage_error)
}
