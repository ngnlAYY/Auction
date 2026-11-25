//! 寄售代管件的应持装备分支：按 position 的持久状态把 Bot 实际持有的寄售件计入应持集合。

/// 应持装备 CTE 的寄售分支（以 `UNION ALL` 开头，接在 `expected` 的最后一段之后）；`$1` 是 world。
///
/// 代管事实只认 `consignment_custody_positions.state`：`in_custody`（已取得）、`committed`
/// （已成交、尚未寄出）与 `reconcile`（位置待核对，仍保留原资产责任）表示实物在 Bot 手里，原因记
/// `consignment_<状态>`。`awaiting_take`（尚未取得）、`in_transit`/`handed_over`（已离开
/// Armory）与季末终态 `season_ended`/`lost`（不再有代管责任）都不计入，季末关闭事务写下终态后
/// 该件即退出应持集合。寄售义务的装备身份在 `custody_equipment_id`、`equipment_id` 恒空，
/// 义务分支的 `JOIN equipment` 天然不含寄售行，因此同一件寄售装备只由本分支计一次。
/// 报表只读，不授权任何资产动作（对账报告 `asset_actions_authorized` 恒为 false）。
macro_rules! consignment_custody_branch {
    () => {
        "
    UNION ALL
    SELECT position.canonical_eid AS external_equipment_id,
           'consignment_' || position.state AS reason
    FROM public.consignment_custody_positions position
    WHERE position.world_id = $1
      AND position.state IN ('in_custody', 'committed', 'reconcile')"
    };
}

pub(super) use consignment_custody_branch;
