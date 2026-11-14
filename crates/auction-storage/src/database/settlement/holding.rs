//! 结算持有时长公式的唯一定义：获胜者在 lot 上的累计领先纳秒，结算锁内据此冻结 `hold_ppm`。

/// 获胜者在该 lot 上的累计领先纳秒（`bigint`；无领先者或领先者无竞价时为 0），`$lot` 是调用方
/// FROM 中 lot 行的别名。每笔竞价按接受顺序持有到下一笔竞价，截断到 lot 有效截止。
///
/// 结算锁内用它计算冻结经济事实的输入；结算报告、公开补偿页与论坛快照用同一片段重放展示值。
/// 竞价事实只追加，结算后 lot 的领先者与有效截止不再变化，重放结果与结算时一致；任一处单独
/// 改排序键或截断口径，展示的持有时长就会偏离已冻结的 `hold_ppm`。
macro_rules! leader_held_nanos {
    ($lot:literal) => {
        concat!(
            "(SELECT COALESCE(SUM(GREATEST(0, LEAST(
          EXTRACT(EPOCH FROM (COALESCE(b.next_at, ",
            $lot,
            ".effective_end_at) - b.created_at)) * 1000000000,
          EXTRACT(EPOCH FROM (",
            $lot,
            ".effective_end_at - b.created_at)) * 1000000000
        )))::bigint, 0)
        FROM (
            SELECT bid.bidder_user_id, bid.created_at,
                   LEAD(bid.created_at) OVER (
                       ORDER BY bid.accepted_state_version, bid.fact_index, bid.id
                   ) AS next_at
            FROM public.bids AS bid
            WHERE bid.lot_id = ",
            $lot,
            ".id
        ) AS b
        WHERE b.bidder_user_id = ",
            $lot,
            ".leader_user_id)"
        )
    };
}

pub(crate) use leader_held_nanos;

#[cfg(test)]
mod tests {
    #[test]
    fn formula_binds_every_lot_reference_to_the_caller_alias() {
        let settlement = leader_held_nanos!("l");
        let public = leader_held_nanos!("lot");
        assert_eq!(settlement.replace("l.", "lot."), public);
        for column in ["effective_end_at", "id", "leader_user_id"] {
            assert!(public.contains(&format!("lot.{column}")), "{column}");
        }
        assert!(!settlement.contains("lot."), "{settlement}");
    }
}
