//! 已成功发送的 MM/PM 被超提醒距有效截止的余量。
//! 先按成功完成时间入口限界，再关联通知/场次；不先扫描稀疏目标。

use auction_application::sli::{
    SliDeadlineState, SliOutbidChannel, SliOutbidMargin, SliSnapshotError, SliWorld,
};
use sqlx::{PgConnection, Row};

use super::{Window, count, ensure_complete, ensure_valid, storage, world_index};

/// MoogleMail 提醒候选：窗口内成功投递关联到通知，并带 Sent 证明。
macro_rules! mm_candidates {
    () => {
        "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.completed_at,notification.lot_id,
               public.delivery_has_frozen_sent_proof(
                   delivery.id,delivery.result_metadata,delivery.external_reference) AS proof_ok
        FROM window_deliveries delivery
        JOIN public.lot_watch_notifications notification
          ON notification.delivery_id=delivery.id
    )"
    };
}

/// 论坛私信提醒候选：私信队列没有独立的 Sent 证明，成功即计入样本。
macro_rules! pm_candidates {
    () => {
        "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.notification_id,delivery.completed_at
        FROM public.lot_watch_pm_deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.notification_id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.completed_at,notification.lot_id
        FROM window_deliveries delivery
        JOIN public.lot_watch_notifications notification
          ON notification.id=delivery.notification_id
    )"
    };
}

/// 两个渠道共用的完整性与关联校验。
macro_rules! count_tail {
    () => {
        "
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates,
           count(*) FILTER (WHERE lot.id IS NULL OR auction.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.lots lot ON lot.id=candidate.lot_id
    LEFT JOIN public.auctions auction ON auction.id=lot.auction_id"
    };
}

const MM_COUNT: &str = concat!(mm_candidates!(), count_tail!());
const PM_COUNT: &str = concat!(pm_candidates!(), count_tail!());

const MM_AGGREGATE: &str = concat!(
    mm_candidates!(),
    "
    SELECT auction.world_id,
           CASE WHEN auction.status IN ('settling','settled','cancelled')
                THEN 'final' ELSE 'provisional' END AS deadline_state,
           count(*) FILTER (WHERE candidate.proof_ok) AS samples,
           percentile_cont(0.01) WITHIN GROUP (
               ORDER BY extract(epoch FROM (lot.effective_end_at-candidate.completed_at))
           ) FILTER (WHERE candidate.proof_ok) AS p01,
           percentile_cont(0.05) WITHIN GROUP (
               ORDER BY extract(epoch FROM (lot.effective_end_at-candidate.completed_at))
           ) FILTER (WHERE candidate.proof_ok) AS p05,
           count(*) FILTER (
               WHERE candidate.proof_ok AND lot.effective_end_at <= candidate.completed_at
           ) AS non_positive,
           count(*) FILTER (WHERE NOT candidate.proof_ok) AS unknown
    FROM candidates candidate
    JOIN public.lots lot ON lot.id=candidate.lot_id
    JOIN public.auctions auction ON auction.id=lot.auction_id
    GROUP BY auction.world_id,deadline_state",
);

const PM_AGGREGATE: &str = concat!(
    pm_candidates!(),
    "
    SELECT auction.world_id,
           CASE WHEN auction.status IN ('settling','settled','cancelled')
                THEN 'final' ELSE 'provisional' END AS deadline_state,
           count(*) AS samples,
           percentile_cont(0.01) WITHIN GROUP (
               ORDER BY extract(epoch FROM (lot.effective_end_at-candidate.completed_at))
           ) AS p01,
           percentile_cont(0.05) WITHIN GROUP (
               ORDER BY extract(epoch FROM (lot.effective_end_at-candidate.completed_at))
           ) AS p05,
           count(*) FILTER (
               WHERE lot.effective_end_at <= candidate.completed_at
           ) AS non_positive
    FROM candidates candidate
    JOIN public.lots lot ON lot.id=candidate.lot_id
    JOIN public.auctions auction ON auction.id=lot.auction_id
    GROUP BY auction.world_id,deadline_state",
);

/// 下标依次为 world、渠道、截止状态。
type Margins = [[[SliOutbidMargin; SliDeadlineState::ALL.len()]; SliOutbidChannel::ALL.len()];
    SliWorld::ALL.len()];

pub(super) async fn read(
    connection: &mut PgConnection,
    window: Window,
) -> Result<Margins, SliSnapshotError> {
    let mut result = Margins::default();
    for channel in SliOutbidChannel::ALL {
        let (count_sql, aggregate_sql) = match channel {
            SliOutbidChannel::Mm => (MM_COUNT, MM_AGGREGATE),
            SliOutbidChannel::Pm => (PM_COUNT, PM_AGGREGATE),
        };
        let totals = window.one(connection, count_sql).await?;
        ensure_complete(&totals, &["window_count", "candidates"])?;
        ensure_valid(&totals)?;
        for row in window.all(connection, aggregate_sql).await? {
            let world = world_index(&row)?;
            let state: String = row.try_get("deadline_state").map_err(storage)?;
            let state = match state.as_str() {
                "provisional" => 0,
                "final" => 1,
                _ => return Err(SliSnapshotError::InvalidData),
            };
            result[world][channel.index()][state] = SliOutbidMargin {
                samples: count(&row, "samples")?,
                p01: row.try_get("p01").map_err(storage)?,
                p05: row.try_get("p05").map_err(storage)?,
                non_positive: count(&row, "non_positive")?,
                // 只有 MoogleMail 渠道带 Sent 证明，私信渠道没有 unknown 口径。
                unknown: match channel {
                    SliOutbidChannel::Mm => count(&row, "unknown")?,
                    SliOutbidChannel::Pm => 0,
                },
            };
        }
    }
    Ok(result)
}
