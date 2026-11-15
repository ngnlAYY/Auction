//! Public 补偿池 PostgreSQL 查询 owner。

// SQL 片段以字面量宏定义，供 `concat!` 组合成静态查询；只在本 owner 内共享。累计领先时长
// 由结算 owner 的 `leader_held_nanos!` 定义。

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
