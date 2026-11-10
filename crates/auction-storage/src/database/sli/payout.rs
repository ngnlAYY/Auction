//! 首次有效付款证明到卖方货款发送成功的窗口。
//! Isekai 的 seller_payout 前置是 winner_equipment_handoff，付款证明在它的前置义务上。

use auction_application::sli::SliSnapshotError;
use sqlx::PgConnection;

use super::{
    PERCENTILE_COLUMNS, Window, WorldWindows, count, ensure_complete, ensure_valid, flow_window,
    world_index,
};

/// 窗口内的卖方货款投递及其 Sent 证明。
macro_rules! candidates {
    () => {
        "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.origin_kind,delivery.payload,delivery.payload_world,
               delivery.payload_operation,delivery.payload_template,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidates AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,
               public.delivery_has_frozen_sent_proof(
                   delivery.id,delivery.result_metadata,delivery.external_reference) AS proof_ok
        FROM window_deliveries delivery
        WHERE delivery.origin_kind='settlement'
          AND delivery.payload_template='settlement_seller_payout'
    )"
    };
}

/// 货款义务 → 前置义务 → 买方付款义务 → 付款证明的关联链。
/// `$join` 在计数语句里是 LEFT JOIN（统计断链候选），其余语句是内连接；
/// `$proof_join` 决定付款证明缺失时候选是保留（计入 unknown）还是剔除（不进分位）。
macro_rules! payment_chain {
    ($join:literal, $proof_join:literal) => {
        concat!(
            "
    FROM candidates candidate
    ",
            $join,
            " public.settlement_obligation_fulfillments fulfillment
      ON fulfillment.delivery_id=candidate.id
    ",
            $join,
            " public.settlement_obligations payout
      ON payout.id=fulfillment.obligation_id AND payout.obligation_kind='seller_payout'
    ",
            $join,
            " public.settlement_obligations link
      ON link.id=payout.prerequisite_obligation_id
    LEFT JOIN public.settlement_obligations handoff
      ON handoff.id=link.id AND handoff.obligation_kind='winner_equipment_handoff'
    ",
            $join,
            " public.settlement_obligations payment
      ON payment.id=CASE WHEN link.obligation_kind='winner_equipment_handoff'
                         THEN handoff.prerequisite_obligation_id ELSE link.id END
     AND payment.obligation_kind IN ('winner_equipment_cod','winner_payment_carrier_cod')
    ",
            $proof_join,
            " public.settlement_obligation_fulfillments proof
      ON proof.obligation_id=payment.id AND proof.proof_observed_at IS NOT NULL"
        )
    };
}

const PAYOUT_COUNT: &str = concat!(
    candidates!(),
    "
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates,
           count(*) FILTER (
               WHERE payout.id IS NULL OR link.id IS NULL OR payment.id IS NULL
           ) AS invalid",
    payment_chain!("LEFT JOIN", "LEFT JOIN"),
);

const PAYOUT_UNKNOWN: &str = concat!(
    candidates!(),
    "
    SELECT payout.world_id,count(DISTINCT candidate.id) AS unknown",
    payment_chain!("JOIN", "LEFT JOIN"),
    "
    WHERE NOT candidate.proof_ok OR proof.proof_observed_at IS NULL
    GROUP BY payout.world_id",
);

const PAYOUT_AGGREGATE: &str = concat!(
    candidates!(),
    "
    SELECT payout.world_id,count(*) FILTER (WHERE candidate.proof_ok) AS samples,
           percentile_cont(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-proof.proof_observed_at))
           ) FILTER (WHERE candidate.proof_ok) AS p50,
           percentile_cont(0.95) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-proof.proof_observed_at))
           ) FILTER (WHERE candidate.proof_ok) AS p95,
           percentile_cont(0.99) WITHIN GROUP (
               ORDER BY extract(epoch FROM (candidate.completed_at-proof.proof_observed_at))
           ) FILTER (WHERE candidate.proof_ok) AS p99",
    payment_chain!("JOIN", "JOIN"),
    "
    GROUP BY payout.world_id",
);

pub(super) async fn read(
    connection: &mut PgConnection,
    window: Window,
) -> Result<WorldWindows, SliSnapshotError> {
    let totals = window.one(connection, PAYOUT_COUNT).await?;
    ensure_complete(&totals, &["window_count", "candidates"])?;
    ensure_valid(&totals)?;
    let mut windows = WorldWindows::default();
    for row in window.all(connection, PAYOUT_UNKNOWN).await? {
        windows[world_index(&row)?].unknown = count(&row, "unknown")?;
    }
    for row in window.all(connection, PAYOUT_AGGREGATE).await? {
        let index = world_index(&row)?;
        windows[index] = flow_window(&row, PERCENTILE_COLUMNS, windows[index].unknown)?;
    }
    Ok(windows)
}
