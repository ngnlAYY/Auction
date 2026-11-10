//! Public 补偿池 PostgreSQL 查询 owner。

// SQL 片段以字面量宏定义，供 `concat!` 组合成静态查询；只在本 owner 内共享。

/// 获胜者在该 lot 上的累计领先纳秒；与结算锁内计算同一公式，`lot` 别名由调用方提供。
/// 竞价事实只追加，结算后 lot 的领先者与有效截止不再变化，重放结果与结算时一致。
macro_rules! leader_held_nanos {
    () => {
        "COALESCE(
                (SELECT SUM(GREATEST(0, LEAST(
                    EXTRACT(EPOCH FROM (COALESCE(b.next_at, lot.effective_end_at) - b.created_at)) * 1000000000,
                    EXTRACT(EPOCH FROM (lot.effective_end_at - b.created_at)) * 1000000000
                )))
                FROM (
                    SELECT bid.bidder_user_id, bid.created_at,
                           LEAD(bid.created_at) OVER (
                               ORDER BY bid.accepted_state_version, bid.fact_index, bid.id
                           ) AS next_at
                    FROM public.bids AS bid
                    WHERE bid.lot_id = lot.id
                ) AS b
                WHERE b.bidder_user_id = lot.leader_user_id)::bigint,
                0
            )"
    };
}

/// 批次摘要的公共列；`settlement`、`a` 与 `round_totals!` 的别名由调用方 FROM 提供。
macro_rules! round_columns {
    () => {
        "a.id AS auction_id,
            a.world_id,
            a.sequence AS auction_sequence,
            settlement.policy_mode,
            settlement.opening_pool_balance AS opening_balance,
            totals.tax_income,
            settlement.discount_demand,
            totals.compensation_out,
            settlement.closing_pool_balance AS closing_balance,
            totals.movement_count,
            reversals.reversal_count,
            reversals.reversal_amount,
            settlement.settled_at"
    };
}

/// 按 `settlement.auction_id` 汇总账本与冲正；账本合计取自只追加流水而非结算计划。
macro_rules! round_totals {
    () => {
        "JOIN LATERAL (
             SELECT
                 count(*)::bigint AS movement_count,
                 coalesce(sum(ledger.amount) FILTER (WHERE ledger.movement_kind = 'tax'), 0)::bigint
                     AS tax_income,
                 coalesce(
                     -(sum(ledger.amount) FILTER (WHERE ledger.movement_kind = 'compensation')),
                     0
                 )::bigint AS compensation_out
             FROM public.compensation_ledger AS ledger
             WHERE ledger.auction_id = settlement.auction_id
         ) AS totals ON true
         JOIN LATERAL (
             SELECT
                 count(*)::bigint AS reversal_count,
                 coalesce(sum(reversal.compensation_amount - reversal.tax_reversed), 0)::bigint
                     AS reversal_amount
             FROM public.compensation_pool_reversals AS reversal
             WHERE reversal.auction_id = settlement.auction_id
         ) AS reversals ON true"
    };
}

mod lots;
mod movements;
mod rounds;

pub(super) use lots::lots;
pub(super) use movements::movements;
pub(super) use rounds::list;
use rounds::load_round;
