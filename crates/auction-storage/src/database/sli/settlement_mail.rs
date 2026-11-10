//! 场次结算到首封/末封付款邮件成功证明的窗口。
//! 成功样本按付款邮件完成终点落窗；未完成覆盖独立枚举全部结算场次。
//! 所有入口先按物理时间索引 MATERIALIZED LIMIT 10001，再分类；义务扇出另有哨兵。

use auction_application::sli::{SLI_CANDIDATE_LIMIT, SliSnapshotError};
use sqlx::{PgConnection, Row};

use super::{Window, WorldWindows, count, ensure_complete, flow_window, storage, world_index};

/// 窗口内有付款邮件成功投递的结算场次。
macro_rules! candidate_auctions {
    () => {
        "
    WITH window_deliveries AS MATERIALIZED (
        SELECT delivery.id,delivery.completed_at,delivery.payload,delivery.payload_world,
               delivery.payload_operation,delivery.payload_template,delivery.origin_kind,
               delivery.result_metadata,delivery.external_reference
        FROM public.deliveries delivery
        WHERE delivery.status='succeeded'
          AND delivery.completed_at >= $1 AND delivery.completed_at <= $2
        ORDER BY delivery.completed_at,delivery.id
        LIMIT $3
    ),
    candidate_auctions AS MATERIALIZED (
        SELECT DISTINCT settlement.auction_id,settlement.settled_at,auction.world_id
        FROM window_deliveries delivery
        JOIN public.settlement_obligation_fulfillments fulfillment
          ON fulfillment.delivery_id=delivery.id
        JOIN public.settlement_obligations obligation
          ON obligation.id=fulfillment.obligation_id
        JOIN public.auction_settlements settlement
          ON settlement.auction_id=obligation.auction_id
        JOIN public.auctions auction ON auction.id=obligation.auction_id
        WHERE delivery.origin_kind='settlement'
          AND delivery.payload_template IN
              ('settlement_winner_equipment_cod','settlement_payment_carrier')
          AND obligation.obligation_kind IN
              ('winner_equipment_cod','winner_payment_carrier_cod')
    )"
    };
}

/// 候选场次的全部外部付款义务；Bot 内部履约的义务不产生付款邮件。
macro_rules! payments {
    () => {
        concat!(
            candidate_auctions!(),
            ",
    payments AS MATERIALIZED (
        SELECT obligation.auction_id,obligation.id AS obligation_id,
               candidate.settled_at,candidate.world_id
        FROM candidate_auctions candidate
        JOIN public.settlement_obligations obligation
          ON obligation.auction_id=candidate.auction_id
        WHERE obligation.obligation_kind IN
              ('winner_equipment_cod','winner_payment_carrier_cod')
          AND NOT EXISTS (
              SELECT 1 FROM public.settlement_obligation_fulfillments internal
              WHERE internal.obligation_id=obligation.id
                AND internal.execution_state='satisfied'
                AND internal.proof_kind='bot_internal_v1'
          )
        LIMIT $3
    )"
        )
    };
}

const SETTLEMENT_COUNT: &str = concat!(
    candidate_auctions!(),
    "
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           count(*) AS candidates
    FROM candidate_auctions",
);

const SETTLEMENT_PAYMENT_COUNT: &str = concat!(
    payments!(),
    "
    SELECT (SELECT count(*) FROM window_deliveries) AS window_count,
           (SELECT count(*) FROM candidate_auctions) AS candidate_auctions,
           count(*) AS payment_count
    FROM payments",
);

const SETTLEMENT_AGGREGATE: &str = concat!(
    payments!(),
    ",
    fulfilled AS (
        SELECT payment.auction_id,payment.world_id,payment.settled_at,payment.obligation_id,
               min(delivery.completed_at) FILTER (
                   WHERE delivery.status='succeeded'
                     AND public.delivery_has_frozen_sent_proof(
                         delivery.id,delivery.result_metadata,delivery.external_reference)
               ) AS first_at,
               max(delivery.completed_at) FILTER (
                   WHERE delivery.status='succeeded'
                     AND public.delivery_has_frozen_sent_proof(
                         delivery.id,delivery.result_metadata,delivery.external_reference)
               ) AS last_at,
               -- 任一付款投递缺强证明即视为该义务证明不全；无投递也必须为 false。
               COALESCE(bool_and(COALESCE(
                   public.delivery_has_frozen_sent_proof(
                       delivery.id,delivery.result_metadata,delivery.external_reference),
                   false)),false) AS all_proof,
               -- 无 fulfillment/delivery 的义务必须视为未成功；空组 bool_or 为 NULL，
               -- bool_and 会忽略 NULL，不能据此宣称末封已完成。
               COALESCE(bool_or(
                   delivery.status='succeeded'
                   AND public.delivery_has_frozen_sent_proof(
                       delivery.id,delivery.result_metadata,delivery.external_reference)
               ),false) AS has_success
        FROM payments payment
        LEFT JOIN public.settlement_obligation_fulfillments fulfillment
          ON fulfillment.obligation_id=payment.obligation_id
        LEFT JOIN public.deliveries delivery
          ON delivery.id=fulfillment.delivery_id
        GROUP BY payment.auction_id,payment.world_id,payment.settled_at,payment.obligation_id
    ),
    per_auction AS (
        SELECT world_id,auction_id,settled_at,
               bool_and(all_proof) AS all_proof,
               bool_and(has_success) AS all_success,
               CASE WHEN bool_and(all_proof) THEN min(first_at) END AS first_at,
               CASE WHEN bool_and(all_proof) AND bool_and(has_success)
                    THEN max(last_at) END AS last_at
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
           count(*) FILTER (WHERE NOT all_proof) AS unknown
    FROM per_auction
    GROUP BY world_id",
);

const FIRST_COLUMNS: [&str; 4] = ["first_samples", "first_p50", "first_p95", "first_p99"];
const LAST_COLUMNS: [&str; 4] = ["last_samples", "last_p50", "last_p95", "last_p99"];

pub(super) async fn read(
    connection: &mut PgConnection,
    window: Window,
) -> Result<(WorldWindows, WorldWindows), SliSnapshotError> {
    let totals = window.one(connection, SETTLEMENT_COUNT).await?;
    ensure_complete(&totals, &["window_count", "candidates"])?;
    let payments = window.one(connection, SETTLEMENT_PAYMENT_COUNT).await?;
    let payment_count: i64 = payments.try_get("payment_count").map_err(storage)?;
    if payment_count > SLI_CANDIDATE_LIMIT as i64 {
        return Err(SliSnapshotError::Limited);
    }
    let mut first = WorldWindows::default();
    let mut last = WorldWindows::default();
    for row in window.all(connection, SETTLEMENT_AGGREGATE).await? {
        let index = world_index(&row)?;
        let unknown = count(&row, "unknown")?;
        first[index] = flow_window(&row, FIRST_COLUMNS, unknown)?;
        last[index] = flow_window(&row, LAST_COLUMNS, unknown)?;
    }
    Ok((first, last))
}
