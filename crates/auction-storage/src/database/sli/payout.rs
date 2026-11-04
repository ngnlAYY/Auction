//! 首次有效付款证明到卖方货款发送成功的窗口。
//! Isekai 的 seller_payout 前置是 winner_equipment_handoff，付款证明在它的前置义务上。

use auction_application::{
    SLI_CANDIDATE_LIMIT, SLI_WINDOW_SECONDS, SliFlowWindow, SliSnapshotError, SliWorld,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};

use super::{candidate_limit, storage};

const PAYOUT_COUNT: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.origin_kind,delivery.payload
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at
        FROM window_deliveries delivery
        WHERE delivery.origin_kind='settlement'
          AND delivery.payload->>'template'='settlement_seller_payout'
    )
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates,
           count(*) FILTER (
               WHERE payout.id IS NULL OR link.id IS NULL OR payment.id IS NULL
           ) AS invalid
    FROM candidates candidate
    LEFT JOIN public.settlement_obligation_fulfillments fulfillment
      ON fulfillment.delivery_id=candidate.id
    LEFT JOIN public.settlement_obligations payout
      ON payout.id=fulfillment.obligation_id AND payout.obligation_kind='seller_payout'
    LEFT JOIN public.settlement_obligations link
      ON link.id=payout.prerequisite_obligation_id
    LEFT JOIN public.settlement_obligations handoff
      ON handoff.id=link.id AND handoff.obligation_kind='winner_equipment_handoff'
    LEFT JOIN public.settlement_obligations payment
      ON payment.id=CASE WHEN link.obligation_kind='winner_equipment_handoff'
                         THEN handoff.prerequisite_obligation_id ELSE link.id END
     AND payment.obligation_kind IN ('winner_equipment_cod','winner_payment_carrier_cod')
    LEFT JOIN public.settlement_obligation_fulfillments proof
      ON proof.obligation_id=payment.id AND proof.proof_observed_at IS NOT NULL";

const PAYOUT_UNKNOWN: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.origin_kind,delivery.payload
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at
        FROM window_deliveries delivery
        WHERE delivery.origin_kind='settlement'
          AND delivery.payload->>'template'='settlement_seller_payout'
    )
    SELECT payout.world_id,count(*) AS unknown
    FROM candidates candidate
    JOIN public.settlement_obligation_fulfillments fulfillment
      ON fulfillment.delivery_id=candidate.id
    JOIN public.settlement_obligations payout
      ON payout.id=fulfillment.obligation_id AND payout.obligation_kind='seller_payout'
    JOIN public.settlement_obligations link
      ON link.id=payout.prerequisite_obligation_id
    LEFT JOIN public.settlement_obligations handoff
      ON handoff.id=link.id AND handoff.obligation_kind='winner_equipment_handoff'
    JOIN public.settlement_obligations payment
      ON payment.id=CASE WHEN link.obligation_kind='winner_equipment_handoff'
                         THEN handoff.prerequisite_obligation_id ELSE link.id END
     AND payment.obligation_kind IN ('winner_equipment_cod','winner_payment_carrier_cod')
    LEFT JOIN public.settlement_obligation_fulfillments proof
      ON proof.obligation_id=payment.id AND proof.proof_observed_at IS NOT NULL
    WHERE proof.proof_observed_at IS NULL
    GROUP BY payout.world_id";

const PAYOUT_AGGREGATE: &str = "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.origin_kind,delivery.payload
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at
        FROM window_deliveries delivery
        WHERE delivery.origin_kind='settlement'
          AND delivery.payload->>'template'='settlement_seller_payout'
    )
    SELECT payout.world_id,count(*) AS samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-proof.proof_observed_at))
           ) AS p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-proof.proof_observed_at))
           ) AS p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-proof.proof_observed_at))
           ) AS p99
    FROM candidates candidate
    JOIN public.settlement_obligation_fulfillments fulfillment
      ON fulfillment.delivery_id=candidate.id
    JOIN public.settlement_obligations payout
      ON payout.id=fulfillment.obligation_id AND payout.obligation_kind='seller_payout'
    JOIN public.settlement_obligations link
      ON link.id=payout.prerequisite_obligation_id
    LEFT JOIN public.settlement_obligations handoff
      ON handoff.id=link.id AND handoff.obligation_kind='winner_equipment_handoff'
    JOIN public.settlement_obligations payment
      ON payment.id=CASE WHEN link.obligation_kind='winner_equipment_handoff'
                         THEN handoff.prerequisite_obligation_id ELSE link.id END
     AND payment.obligation_kind IN ('winner_equipment_cod','winner_payment_carrier_cod')
    JOIN public.settlement_obligation_fulfillments proof
      ON proof.obligation_id=payment.id AND proof.proof_observed_at IS NOT NULL
    GROUP BY payout.world_id";

pub(super) async fn read(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
) -> Result<[SliFlowWindow; SliWorld::ALL.len()], SliSnapshotError> {
    let since = now
        .checked_sub_signed(chrono::Duration::seconds(SLI_WINDOW_SECONDS))
        .ok_or(SliSnapshotError::InvalidData)?;
    let row = sqlx::query(PAYOUT_COUNT)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
        .fetch_one(&mut *connection)
        .await
        .map_err(storage)?;
    let window_count: i64 = row.try_get("window_count").map_err(storage)?;
    let candidate_count: i64 = row.try_get("candidates").map_err(storage)?;
    let invalid: i64 = row.try_get("invalid").map_err(storage)?;
    if window_count > SLI_CANDIDATE_LIMIT as i64 || candidate_count > SLI_CANDIDATE_LIMIT as i64 {
        return Err(SliSnapshotError::Limited);
    }
    if invalid > 0 {
        return Err(SliSnapshotError::InvalidData);
    }
    let mut windows = [SliFlowWindow::default(); SliWorld::ALL.len()];
    for row in sqlx::query(PAYOUT_UNKNOWN)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
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
        let unknown: i64 = row.try_get("unknown").map_err(storage)?;
        windows[index].unknown = u64::try_from(unknown).map_err(|error| {
            let _ = error;
            SliSnapshotError::InvalidData
        })?;
    }
    for row in sqlx::query(PAYOUT_AGGREGATE)
        .bind(since)
        .bind(now)
        .bind(candidate_limit())
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
        let samples: i64 = row.try_get("samples").map_err(storage)?;
        windows[index] = SliFlowWindow {
            samples: u64::try_from(samples).map_err(|error| {
                let _ = error;
                SliSnapshotError::InvalidData
            })?,
            p50: row.try_get("p50").map_err(storage)?,
            p95: row.try_get("p95").map_err(storage)?,
            p99: row.try_get("p99").map_err(storage)?,
            unknown: windows[index].unknown,
            incomplete: 0,
            oldest_incomplete_seconds: None,
            incomplete_available: false,
        };
    }
    Ok(windows)
}
