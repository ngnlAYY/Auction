//! 场次结算到首封/末封付款邮件成功证明的窗口。
//! 候选按付款邮件完成终点落窗；起点 settled_at 可以早于七天。

use auction_application::{
    SLI_CANDIDATE_LIMIT, SLI_WINDOW_SECONDS, SliFlowWindow, SliSnapshotError, SliWorld,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use super::{candidate_limit, storage};

const SETTLEMENT_COUNT: &str = "
    WITH candidate_auctions AS MATERIALIZED (
        SELECT DISTINCT settlement.auction_id,settlement.settled_at,auction.world_id
        FROM public.deliveries delivery
        JOIN public.settlement_obligation_fulfillments fulfillment
          ON fulfillment.delivery_id=delivery.id
        JOIN public.settlement_obligations obligation
          ON obligation.id=fulfillment.obligation_id
        JOIN public.auction_settlements settlement
          ON settlement.auction_id=obligation.auction_id
        JOIN public.auctions auction ON auction.id=obligation.auction_id
        WHERE delivery.status='succeeded'
          AND delivery.origin_kind='settlement'
          AND delivery.payload->>'template' IN
              ('settlement_winner_equipment_cod','settlement_payment_carrier',
               'settlement_equipment_handoff')
          AND obligation.obligation_kind IN
              ('winner_equipment_cod','winner_payment_carrier_cod','winner_equipment_handoff')
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY settlement.settled_at,settlement.auction_id
        LIMIT $3
    )
    SELECT count(*) AS candidates FROM candidate_auctions";

const SETTLEMENT_AGGREGATE: &str = "
    WITH candidate_auctions AS MATERIALIZED (
        SELECT DISTINCT settlement.auction_id,settlement.settled_at,auction.world_id
        FROM public.deliveries delivery
        JOIN public.settlement_obligation_fulfillments fulfillment
          ON fulfillment.delivery_id=delivery.id
        JOIN public.settlement_obligations obligation
          ON obligation.id=fulfillment.obligation_id
        JOIN public.auction_settlements settlement
          ON settlement.auction_id=obligation.auction_id
        JOIN public.auctions auction ON auction.id=obligation.auction_id
        WHERE delivery.status='succeeded'
          AND delivery.origin_kind='settlement'
          AND delivery.payload->>'template' IN
              ('settlement_winner_equipment_cod','settlement_payment_carrier',
               'settlement_equipment_handoff')
          AND obligation.obligation_kind IN
              ('winner_equipment_cod','winner_payment_carrier_cod','winner_equipment_handoff')
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY settlement.settled_at,settlement.auction_id
        LIMIT $3
    ),
    payments AS (
        SELECT obligation.auction_id,obligation.id AS obligation_id,
               candidate.settled_at,candidate.world_id
        FROM candidate_auctions candidate
        JOIN public.settlement_obligations obligation
          ON obligation.auction_id=candidate.auction_id
        WHERE obligation.obligation_kind IN
              ('winner_equipment_cod','winner_payment_carrier_cod','winner_equipment_handoff')
    ),
    fulfilled AS (
        SELECT payment.auction_id,payment.world_id,payment.settled_at,payment.obligation_id,
               min(delivery.completed_at) FILTER (WHERE delivery.status='succeeded') AS first_at,
               max(delivery.completed_at) FILTER (WHERE delivery.status='succeeded') AS last_at,
               -- 无 fulfillment/delivery 的义务必须视为未成功；空组 bool_or 为 NULL，
               -- bool_and 会忽略 NULL，不能据此宣称末封已完成。
               COALESCE(bool_or(delivery.status='succeeded'),false) AS has_success
        FROM payments payment
        LEFT JOIN public.settlement_obligation_fulfillments fulfillment
          ON fulfillment.obligation_id=payment.obligation_id
        LEFT JOIN public.deliveries delivery
          ON delivery.id=fulfillment.delivery_id AND delivery.status='succeeded'
        GROUP BY payment.auction_id,payment.world_id,payment.settled_at,payment.obligation_id
    ),
    per_auction AS (
        SELECT world_id,auction_id,settled_at,
               min(first_at) AS first_at,
               CASE WHEN bool_and(has_success) THEN max(last_at) END AS last_at,
               bool_and(has_success) AS all_success
        FROM fulfilled
        GROUP BY world_id,auction_id,settled_at
    )
    SELECT world_id,
           count(*) FILTER (WHERE first_at >= $1 AND first_at <= $2) AS first_samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (first_at-settled_at))
           ) FILTER (WHERE first_at >= $1 AND first_at <= $2) AS first_p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (first_at-settled_at))
           ) FILTER (WHERE first_at >= $1 AND first_at <= $2) AS first_p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (first_at-settled_at))
           ) FILTER (WHERE first_at >= $1 AND first_at <= $2) AS first_p99,
           count(*) FILTER (WHERE last_at >= $1 AND last_at <= $2) AS last_samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (last_at-settled_at))
           ) FILTER (WHERE last_at >= $1 AND last_at <= $2) AS last_p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (last_at-settled_at))
           ) FILTER (WHERE last_at >= $1 AND last_at <= $2) AS last_p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (last_at-settled_at))
           ) FILTER (WHERE last_at >= $1 AND last_at <= $2) AS last_p99,
           count(*) FILTER (WHERE NOT all_success) AS incomplete,
           (extract(epoch FROM ($2 - min(settled_at) FILTER (WHERE NOT all_success))))
               ::double precision AS oldest_incomplete_seconds
    FROM per_auction
    GROUP BY world_id";

const SETTLEMENT_ALL_COUNT: &str = "
    WITH settlements AS MATERIALIZED (
        SELECT settlement.auction_id
        FROM public.auction_settlements settlement
        ORDER BY settlement.settled_at,settlement.auction_id
        LIMIT $1
    )
    SELECT count(*) AS candidates FROM settlements";

const SETTLEMENT_INCOMPLETE: &str = "
    WITH settlements AS MATERIALIZED (
        SELECT settlement.auction_id,settlement.settled_at,auction.world_id
        FROM public.auction_settlements settlement
        JOIN public.auctions auction ON auction.id=settlement.auction_id
        ORDER BY settlement.settled_at,settlement.auction_id
        LIMIT $1
    ),
    payments AS (
        SELECT obligation.auction_id,obligation.id AS obligation_id,
               settlement.settled_at,settlement.world_id
        FROM settlements settlement
        JOIN public.settlement_obligations obligation
          ON obligation.auction_id=settlement.auction_id
        WHERE obligation.obligation_kind IN
              ('winner_equipment_cod','winner_payment_carrier_cod','winner_equipment_handoff')
    ),
    fulfilled AS (
        SELECT payment.auction_id,payment.world_id,payment.settled_at,payment.obligation_id,
               COALESCE(bool_or(delivery.status='succeeded'),false) AS has_success
        FROM payments payment
        LEFT JOIN public.settlement_obligation_fulfillments fulfillment
          ON fulfillment.obligation_id=payment.obligation_id
        LEFT JOIN public.deliveries delivery
          ON delivery.id=fulfillment.delivery_id AND delivery.status='succeeded'
        GROUP BY payment.auction_id,payment.world_id,payment.settled_at,payment.obligation_id
    ),
    per_auction AS (
        SELECT world_id,auction_id,settled_at,bool_and(has_success) AS all_success
        FROM fulfilled
        GROUP BY world_id,auction_id,settled_at
    )
    SELECT world_id,count(*) AS incomplete,
           (extract(epoch FROM ($2 - min(settled_at))))::double precision
               AS oldest_incomplete_seconds
    FROM per_auction
    WHERE NOT all_success
    GROUP BY world_id";

pub(super) async fn read(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<
    ([SliFlowWindow; SliWorld::ALL.len()], [SliFlowWindow; SliWorld::ALL.len()]),
    SliSnapshotError,
> {
    let since = now
        .checked_sub_signed(chrono::Duration::seconds(SLI_WINDOW_SECONDS))
        .ok_or(SliSnapshotError::InvalidData)?;
    let row = sqlx::query(SETTLEMENT_COUNT)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
        .fetch_one(&mut *connection)
        .await
        .map_err(storage)?;
    let candidates: i64 = row.try_get("candidates").map_err(storage)?;
    if candidates > SLI_CANDIDATE_LIMIT as i64 {
        return Err(SliSnapshotError::Limited);
    }
    let all_count = sqlx::query(SETTLEMENT_ALL_COUNT)
        .bind(candidate_limit())
        .fetch_one(&mut *connection)
        .await
        .map_err(storage)?;
    let all_candidates: i64 = all_count.try_get("candidates").map_err(storage)?;
    if all_candidates > SLI_CANDIDATE_LIMIT as i64 {
        return Err(SliSnapshotError::Limited);
    }
    let rows = sqlx::query(SETTLEMENT_AGGREGATE)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
        .fetch_all(&mut *connection)
        .await
        .map_err(storage)?;
    let mut first = [SliFlowWindow::default(); SliWorld::ALL.len()];
    let mut last = [SliFlowWindow::default(); SliWorld::ALL.len()];
    for row in rows {
        let world: String = row.try_get("world_id").map_err(storage)?;
        let index = match world.as_str() {
            "main" => 0,
            "isekai" => 1,
            _ => return Err(SliSnapshotError::InvalidData),
        };
        let first_samples: i64 = row.try_get("first_samples").map_err(storage)?;
        let last_samples: i64 = row.try_get("last_samples").map_err(storage)?;
        let incomplete: i64 = row.try_get("incomplete").map_err(storage)?;
        first[index] = SliFlowWindow {
            samples: u64::try_from(first_samples).map_err(|error| {
                let _ = error;
                SliSnapshotError::InvalidData
            })?,
            p50: row.try_get("first_p50").map_err(storage)?,
            p95: row.try_get("first_p95").map_err(storage)?,
            p99: row.try_get("first_p99").map_err(storage)?,
            unknown: 0,
            incomplete: 0,
            oldest_incomplete_seconds: None,
        };
        last[index] = SliFlowWindow {
            samples: u64::try_from(last_samples).map_err(|error| {
                let _ = error;
                SliSnapshotError::InvalidData
            })?,
            p50: row.try_get("last_p50").map_err(storage)?,
            p95: row.try_get("last_p95").map_err(storage)?,
            p99: row.try_get("last_p99").map_err(storage)?,
            unknown: 0,
            incomplete: u64::try_from(incomplete).map_err(|error| {
                let _ = error;
                SliSnapshotError::InvalidData
            })?,
            oldest_incomplete_seconds: row.try_get("oldest_incomplete_seconds").map_err(storage)?,
        };
    }
    for row in sqlx::query(SETTLEMENT_INCOMPLETE)
        .bind(candidate_limit())
        .bind(now)
        .fetch_all(&mut *connection)
        .await
        .map_err(storage)?
    {
        let world: String = row.try_get("world_id").map_err(storage)?;
        let index = match world.as_str() {
            "main" => 0,
            "isekai" => 1,
            _ => return Err(SliSnapshotError::InvalidData),
        };
        let incomplete: i64 = row.try_get("incomplete").map_err(storage)?;
        last[index].incomplete = u64::try_from(incomplete).map_err(|error| {
            let _ = error;
            SliSnapshotError::InvalidData
        })?;
        last[index].oldest_incomplete_seconds =
            row.try_get("oldest_incomplete_seconds").map_err(storage)?;
    }
    Ok((first, last))
}
