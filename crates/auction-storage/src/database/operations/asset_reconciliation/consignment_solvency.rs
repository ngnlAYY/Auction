//! 寄售成交的偿付负债：一笔 funded 成交只计一份最大负债，卖款与买家退款互斥（D9 §4.4，统筹方裁定）。
//!
//! 保护不变量 5、6、9：只读、只认持久事实（funded 投影、义务、履约、投递与资金分配），不读缓存，
//! 不授权任何资金动作。Bot 收到买家付款 P 后，只可能付卖家 S（F 为平台手续费，不是负债）或退买家 P，
//! 二者互斥，最大负债取 P 而不是 S+P：
//!
//! - `funded`：卖款尚未寄出（寄出即退款与卖款冲突，由人工核对）时计退款义务的 P；卖款已寄出时只计卖款 S。
//! - `refund_converted`：只计退款义务 P。
//! - `original_claimed`：原件已确领，退款不再可能，只计卖款 S。
//!
//! 未付款（无 funded 投影）的成交 Bot 尚未持有买家款项，不计负债。已满足的义务不计；在途与待付的划分、
//! 余额观察时点口径与拍卖相同（见 `queries::SOLVENCY_WORLD`）；金额按 `COALESCE(dispatch_world,
//! world_id)` 归世界。在途待核对的原因加 `consignment_` 前缀，与拍卖原因区分。

use sqlx::{PgConnection, Row};

use super::query_failed;
use crate::database::settlement::fulfillment::{
    sent_awaiting_collection_sql, sent_unsatisfied_sql,
};

/// 寄售负债的偿付分类查询；`$1` 是 world。返回列与拍卖口径同名，由 `solvency.rs` 逐项并入。
const CONSIGNMENT_SOLVENCY_WORLD: &str = concat!(
    r#"
WITH health AS (
    SELECT balance_observed_at
    FROM public.settlement_recovery_health
    WHERE world_id = $1
), legs AS (
    SELECT funded.contract_id, funded.disposition,
           payout.id AS payout_id, refund.id AS refund_id,
           COALESCE(payout_fulfillment.execution_state, 'pending') AS payout_state,
           COALESCE("#,
    sent_unsatisfied_sql!("payout_fulfillment", "payout_delivery"),
    r#", false) AS payout_sent
    FROM public.consignment_funded_sales funded
    JOIN public.settlement_obligations payout
        ON payout.contract_id = funded.contract_id AND payout.obligation_kind = 'seller_payout'
    LEFT JOIN public.settlement_obligation_fulfillments payout_fulfillment
        ON payout_fulfillment.obligation_id = payout.id
    LEFT JOIN public.deliveries payout_delivery ON payout_delivery.id = payout_fulfillment.delivery_id
    JOIN public.settlement_obligations refund
        ON refund.contract_id = funded.contract_id AND refund.obligation_kind = 'buyer_refund'
), governing AS (
    SELECT contract_id,
           CASE WHEN disposition = 'refund_converted'
                     OR (disposition = 'funded' AND NOT payout_sent AND payout_state <> 'satisfied')
                THEN refund_id ELSE payout_id END AS obligation_id
    FROM legs
), outstanding AS (
    SELECT o.obligation_kind, o.amount,
           COALESCE("#,
    sent_unsatisfied_sql!("f", "d"),
    r#"
               AND allocation.state = 'spent'
               AND d.completed_at <= (SELECT balance_observed_at FROM health), false)
               AS spent_before_balance,
           COALESCE("#,
    sent_awaiting_collection_sql!("f", "d"),
    r#", false) AS awaiting_collection,
           'consignment_obligation_' || o.obligation_kind || '_'
               || COALESCE(f.execution_state, 'pending') AS review_reason
    FROM governing g
    JOIN public.settlement_obligations o ON o.id = g.obligation_id
    LEFT JOIN public.settlement_obligation_fulfillments f ON f.obligation_id = o.id
    LEFT JOIN public.deliveries d ON d.id = f.delivery_id
    LEFT JOIN public.settlement_funding_allocations allocation
        ON allocation.delivery_id = f.delivery_id AND allocation.obligation_id = o.id
    WHERE COALESCE(f.dispatch_world, o.world_id) = $1
      AND COALESCE(f.execution_state, 'pending') <> 'satisfied'
), classified AS (
    SELECT obligation_kind, amount, review_reason,
           NOT spent_before_balance AS pending,
           spent_before_balance AND awaiting_collection AS in_transit,
           spent_before_balance AND NOT awaiting_collection AS in_transit_review
    FROM outstanding
), review AS (
    SELECT review_reason AS reason,
           count(*)::bigint AS item_count,
           COALESCE(sum(amount), 0)::bigint AS credits
    FROM classified
    WHERE in_transit_review
    GROUP BY review_reason
)
SELECT
    (SELECT count(*) FROM classified
        WHERE obligation_kind = 'seller_payout' AND pending) AS pending_seller_payout_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM classified
        WHERE obligation_kind = 'seller_payout' AND pending) AS pending_seller_payout_credits,
    (SELECT count(*) FROM classified
        WHERE obligation_kind = 'buyer_refund' AND pending) AS pending_refund_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM classified
        WHERE obligation_kind = 'buyer_refund' AND pending) AS pending_refund_credits,
    (SELECT count(*) FROM classified
        WHERE obligation_kind = 'seller_payout' AND in_transit) AS in_transit_seller_payout_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM classified
        WHERE obligation_kind = 'seller_payout' AND in_transit) AS in_transit_seller_payout_credits,
    (SELECT count(*) FROM classified
        WHERE obligation_kind = 'buyer_refund' AND in_transit) AS in_transit_refund_count,
    (SELECT COALESCE(sum(amount), 0)::bigint FROM classified
        WHERE obligation_kind = 'buyer_refund' AND in_transit) AS in_transit_refund_credits,
    (SELECT COALESCE(array_agg(reason ORDER BY reason), '{}') FROM review)
        AS in_transit_review_reasons,
    (SELECT COALESCE(array_agg(item_count ORDER BY reason), '{}') FROM review)
        AS in_transit_review_counts,
    (SELECT COALESCE(array_agg(credits ORDER BY reason), '{}') FROM review)
        AS in_transit_review_credits,
    -- 现金退件在余额观察之后才被消费回补：观察到的余额尚不含这笔 Credits，不能据此判定。
    EXISTS (
        SELECT 1
        FROM public.consignment_return_consumptions consumption
        JOIN public.mail_disposition_proofs proof ON proof.id = consumption.proof_id
        WHERE consumption.world_id = $1
          AND consumption.leg IN ('cash_refund', 'cash_payout', 'cash_principal')
          AND proof.proven_at > COALESCE(
              (SELECT balance_observed_at FROM health), '-infinity'::timestamptz)
    ) AS balance_before_take
"#,
);

/// 寄售在某 world 的偿付分类；字段与拍卖口径同义，由调用方并入同一份世界报告。
pub(super) struct ConsignmentSolvency {
    pub(super) pending_seller_payout: (i64, i64),
    pub(super) pending_refund: (i64, i64),
    pub(super) in_transit_seller_payout: (i64, i64),
    pub(super) in_transit_refund: (i64, i64),
    pub(super) review_reasons: Vec<String>,
    pub(super) review_counts: Vec<i64>,
    pub(super) review_credits: Vec<i64>,
    pub(super) balance_before_take: bool,
}

/// 读取寄售在某 world 的负债分类；查询失败关闭为 `reconciliation_query_failed`。
pub(super) async fn consignment_solvency(
    connection: &mut PgConnection,
    world: &str,
) -> Result<ConsignmentSolvency, &'static str> {
    let row = sqlx::query(CONSIGNMENT_SOLVENCY_WORLD)
        .bind(world)
        .fetch_one(connection)
        .await
        .map_err(query_failed)?;
    let pair = |count: &str, credits: &str| -> Result<(i64, i64), &'static str> {
        Ok((row.try_get(count).map_err(query_failed)?, row.try_get(credits).map_err(query_failed)?))
    };
    Ok(ConsignmentSolvency {
        pending_seller_payout: pair(
            "pending_seller_payout_count",
            "pending_seller_payout_credits",
        )?,
        pending_refund: pair("pending_refund_count", "pending_refund_credits")?,
        in_transit_seller_payout: pair(
            "in_transit_seller_payout_count",
            "in_transit_seller_payout_credits",
        )?,
        in_transit_refund: pair("in_transit_refund_count", "in_transit_refund_credits")?,
        review_reasons: row.try_get("in_transit_review_reasons").map_err(query_failed)?,
        review_counts: row.try_get("in_transit_review_counts").map_err(query_failed)?,
        review_credits: row.try_get("in_transit_review_credits").map_err(query_failed)?,
        balance_before_take: row.try_get("balance_before_take").map_err(query_failed)?,
    })
}
