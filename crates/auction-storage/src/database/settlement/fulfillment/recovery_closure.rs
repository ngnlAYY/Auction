//! 恢复记录收口事实的共享 SQL 判定：成交撤销（COD 退回、Admin 确认收回/取消/装备收回）在确认
//! 事务里把不再可执行的义务收口为 `failed`，并在原投递当前 fence 上追加恢复来源的统一处置事实。
//!
//! 退款的资金转换、资产冲突门、内部履约校验与 funding 预览都只认这些精确事实：履约失败码、
//! 处置种类、处置来源恢复记录与义务角色必须彼此对应，处置必须落在原投递的当前 fence 上。任意
//! `failed` 或缺少处置的失败一律不算收口（宁可继续阻断，也不静默放行）。判定只用于准入与报告，
//! 不授权任何资金或资产动作。宏参数是 `settlement_recoveries` 的别名。

/// `$recovery` 的 `equipment_recovered` 收口精确成立：resolution 为 `equipment_recovered`，其 isekai
/// 交付履约已收口为 `failed/recovery_equipment_recovered`，且交付投递当前 fence 上有来源为该恢复
/// 记录的 `recovery_equipment_recovered` 处置。确认事务同时收口从未开始的卖方货款，因此这也是
/// 卖方货款 `recovery_sale_voided` 的收口证明。
macro_rules! equipment_recovered_closure_sql {
    ($recovery:literal) => {
        concat!(
            "(",
            $recovery,
            ".resolution = 'equipment_recovered' AND EXISTS (
                SELECT 1
                FROM public.settlement_obligation_fulfillments AS closed_handoff
                JOIN public.deliveries AS closed_delivery
                  ON closed_delivery.id = closed_handoff.delivery_id
                JOIN public.delivery_dispositions AS closure
                  ON closure.delivery_id = closed_delivery.id
                 AND closure.fencing_version = closed_delivery.fencing_version
                 AND closure.kind = 'recovery_equipment_recovered'
                 AND closure.settlement_recovery_id = ",
            $recovery,
            ".id
                WHERE closed_handoff.obligation_id = ",
            $recovery,
            ".equipment_obligation_id
                  AND closed_handoff.execution_state = 'failed'
                  AND closed_handoff.last_error_code = 'recovery_equipment_recovered'))"
        )
    };
}
pub(crate) use equipment_recovered_closure_sql;
