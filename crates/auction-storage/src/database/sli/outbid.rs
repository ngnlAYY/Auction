//! 已成功发送的 MM/PM 被超提醒距有效截止的余量。
//! 先按成功完成时间入口限界，再关联通知/场次；不先扫描稀疏目标。

use auction_application::{
    SLI_CANDIDATE_LIMIT, SLI_WINDOW_SECONDS, SliDeadlineState, SliOutbidChannel, SliOutbidMargin,
    SliSnapshotError, SliWorld,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use super::{candidate_limit, storage};

const MM_COUNT: &str = "
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
    )
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates,
           count(*) FILTER (WHERE lot.id IS NULL OR auction.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.lots lot ON lot.id=candidate.lot_id
    LEFT JOIN public.auctions auction ON auction.id=lot.auction_id";

const MM_AGGREGATE: &str = "
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
    )
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
    GROUP BY auction.world_id,deadline_state";

const PM_COUNT: &str = "
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
    )
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates,
           count(*) FILTER (WHERE lot.id IS NULL OR auction.id IS NULL) AS invalid
    FROM candidates candidate
    LEFT JOIN public.lots lot ON lot.id=candidate.lot_id
    LEFT JOIN public.auctions auction ON auction.id=lot.auction_id";

const PM_AGGREGATE: &str = "
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
    )
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
    GROUP BY auction.world_id,deadline_state";

pub(super) async fn read(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<
    [[[SliOutbidMargin; SliDeadlineState::ALL.len()]; SliOutbidChannel::ALL.len()];
        SliWorld::ALL.len()],
    SliSnapshotError,
> {
    let since = now
        .checked_sub_signed(chrono::Duration::seconds(SLI_WINDOW_SECONDS))
        .ok_or(SliSnapshotError::InvalidData)?;
    let mut result = [[[SliOutbidMargin::default(); SliDeadlineState::ALL.len()];
        SliOutbidChannel::ALL.len()]; SliWorld::ALL.len()];
    for channel in SliOutbidChannel::ALL {
        let (count_sql, aggregate_sql) = match channel {
            SliOutbidChannel::Mm => (MM_COUNT, MM_AGGREGATE),
            SliOutbidChannel::Pm => (PM_COUNT, PM_AGGREGATE),
        };
        let row = sqlx::query(count_sql)
            .bind(since)
            .bind(now)
            .bind(candidate_limit())
            .fetch_one(&mut *connection)
            .await
            .map_err(storage)?;
        let window_count: i64 = row.try_get("window_count").map_err(storage)?;
        let candidate_count: i64 = row.try_get("candidates").map_err(storage)?;
        let invalid: i64 = row.try_get("invalid").map_err(storage)?;
        if window_count > SLI_CANDIDATE_LIMIT as i64 || candidate_count > SLI_CANDIDATE_LIMIT as i64
        {
            return Err(SliSnapshotError::Limited);
        }
        if invalid > 0 {
            return Err(SliSnapshotError::InvalidData);
        }
        let rows = sqlx::query(aggregate_sql)
            .bind(since)
            .bind(now)
            .bind(candidate_limit())
            .fetch_all(&mut *connection)
            .await
            .map_err(storage)?;
        for row in rows {
            let world: String = row.try_get("world_id").map_err(storage)?;
            let state: String = row.try_get("deadline_state").map_err(storage)?;
            let world = match world.as_str() {
                "main" => 0,
                "isekai" => 1,
                _ => return Err(SliSnapshotError::InvalidData),
            };
            let state = match state.as_str() {
                "provisional" => 0,
                "final" => 1,
                _ => return Err(SliSnapshotError::InvalidData),
            };
            let samples: i64 = row.try_get("samples").map_err(storage)?;
            let non_positive: i64 = row.try_get("non_positive").map_err(storage)?;
            let unknown = match channel {
                SliOutbidChannel::Mm => {
                    let unknown: i64 = row.try_get("unknown").map_err(storage)?;
                    u64::try_from(unknown).map_err(|error| {
                        let _ = error;
                        SliSnapshotError::InvalidData
                    })?
                }
                SliOutbidChannel::Pm => 0,
            };
            result[world][channel.index()][state] = SliOutbidMargin {
                samples: u64::try_from(samples).map_err(|error| {
                    let _ = error;
                    SliSnapshotError::InvalidData
                })?,
                p01: row.try_get("p01").map_err(storage)?,
                p05: row.try_get("p05").map_err(storage)?,
                non_positive: u64::try_from(non_positive).map_err(|error| {
                    let _ = error;
                    SliSnapshotError::InvalidData
                })?,
                unknown,
            };
        }
    }
    Ok(result)
}
