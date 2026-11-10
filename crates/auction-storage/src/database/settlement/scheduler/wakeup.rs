//! 结算 Worker 唤醒提示：只读估算下一次到期或重试时刻，不加锁、不领取，也不改变 workflow 事实。

use chrono::{DateTime, Utc};
use sqlx::Postgres;

use super::map_storage_error;
use crate::database::TrackedPool;

/// 初次领取以全部有效 lot 的最晚截止为到期点；重试与完成补写取冻结 due 与 next_attempt 的较晚值。
/// 只返回严格晚于 `now` 的时刻，领取资格仍以 `claim` 的 Serializable 锁内谓词为准。
pub(super) async fn next_claimable_postgres(
    pool: &TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, auction_application::core::RepositoryError> {
    sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        r#"SELECT MIN(candidate.claimable_at)
           FROM (
               SELECT lot_schedule.due_at AS claimable_at
               FROM public.auctions AS a
               CROSS JOIN LATERAL (
                   SELECT MAX(l.effective_end_at) AS due_at,
                          COUNT(*) AS lot_count,
                          COUNT(*) FILTER (
                              WHERE l.status = 'live' AND l.effective_end_at IS NOT NULL
                          ) AS live_lot_count
                   FROM public.lots AS l
                   WHERE l.auction_id = a.id AND l.status <> 'withdrawn'
               ) AS lot_schedule
               WHERE a.status = 'final'
                 AND a.settlement_workflow_id IS NULL
                 AND lot_schedule.lot_count > 0
                 AND lot_schedule.live_lot_count = lot_schedule.lot_count
               UNION ALL
               SELECT GREATEST(a.settlement_due_at, a.settlement_next_attempt_at)
               FROM public.auctions AS a
               WHERE a.status IN ('settling', 'settled')
                 AND a.settlement_workflow_id IS NOT NULL
                 AND a.settlement_complete_command_id IS NULL
                 AND a.settlement_due_at IS NOT NULL
           ) AS candidate
           WHERE candidate.claimable_at > $1"#,
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(map_storage_error)
}
