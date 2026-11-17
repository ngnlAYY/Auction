//! 恢复记录收口事实的共享 SQL 判定：成交撤销（COD 退回、Admin 确认收回/取消/装备收回）在确认
//! 事务里把不再可执行的义务收口为 `failed`，并在原投递当前 fence 上追加恢复来源的统一处置事实。
//!
//! 退款的资金转换、资产冲突门、内部履约校验、funding 预览与资产对账都只认这些精确事实：履约
//! 失败码、处置种类、处置来源恢复记录与义务角色必须彼此对应，处置必须落在原投递的当前 fence
//! 上。任意 `failed` 或缺少处置的失败一律不算收口（宁可继续阻断或计入，也不静默放行）。判定只
//! 用于准入与报告，不授权任何资金或资产动作。各宏的别名参数分别是 `settlement_recoveries`、
//! `settlement_obligations`、`settlement_obligation_fulfillments` 与 `deliveries` 的别名。

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

/// 被收回的 isekai 交付投递 `$delivery` 有可信 Sent 证明：已成功、远端引用已证明，证明是精确载荷
/// （`sent_exact_payload_v1`）、旧格式引用恢复（`sent_recovered_reference_v1`，只为缺冻结发送尝试的
/// 旧投递生成）或冻结尝试绑定的自动/人工证明。确认 `equipment_recovered` 的收回链、内部退款校验与
/// 资产冲突门共用这一谓词；funding 预览只认确认事务据它写下的精确收口事实。各门曾各写一份证明
/// 集合，旧格式证明能确认却在资产冲突门被拒，合法确认后退款仍无法执行。
macro_rules! recovered_handoff_sent_proof_sql {
    ($delivery:literal) => {
        concat!(
            "(",
            $delivery,
            ".status = 'succeeded' AND ",
            $delivery,
            ".result_metadata->>'external_reference_proven' = 'true' AND (",
            $delivery,
            ".result_metadata->>'proof' IN ('sent_exact_payload_v1', 'sent_recovered_reference_v1')
              OR public.delivery_has_frozen_sent_proof(",
            $delivery,
            ".id, ",
            $delivery,
            ".result_metadata, ",
            $delivery,
            ".external_reference)))"
        )
    };
}
pub(crate) use recovered_handoff_sent_proof_sql;

/// 义务 `$obligation` 的履约 `$fulfillment` 已被恢复收口精确终结：履约为 `failed`，失败码与处置
/// 种类、义务角色对应，处置来源为同一恢复记录且落在被处置投递的当前 fence 上。
///
/// - `cod_returned`（COD 达到冻结退回上限退回卖家，或 Admin 确认收回/取消）：付款、isekai 交付与
///   卖方货款，失败码为 `cod_returned_three_times`、`equipment_returned_to_seller`、
///   `recovery_reclaimed`、`recovery_cancelled` 或 `recovery_sale_voided`；
/// - `recovery_equipment_recovered`（Admin 确认装备收回）：isekai 交付与卖方货款，失败码为
///   `recovery_equipment_recovered` 或 `recovery_sale_voided`。
///
/// 买方退款从不由这些处置收口。
macro_rules! recovery_closed_fulfillment_sql {
    ($obligation:literal, $fulfillment:literal) => {
        concat!(
            "(",
            $fulfillment,
            ".execution_state = 'failed' AND EXISTS (
                SELECT 1
                FROM public.settlement_recoveries AS closing
                JOIN public.delivery_dispositions AS closure
                  ON closure.settlement_recovery_id = closing.id
                JOIN public.deliveries AS closed
                  ON closed.id = closure.delivery_id
                 AND closed.fencing_version = closure.fencing_version
                WHERE closing.resolution IS NOT NULL
                  AND ((closure.kind = 'cod_returned'
                        AND ",
            $fulfillment,
            ".last_error_code IN (
                            'cod_returned_three_times', 'equipment_returned_to_seller',
                            'recovery_reclaimed', 'recovery_cancelled', 'recovery_sale_voided')
                        AND ",
            $obligation,
            ".id IN (closing.payment_obligation_id, closing.equipment_obligation_id,
                                 closing.seller_payout_obligation_id))
                    OR (closure.kind = 'recovery_equipment_recovered'
                        AND closing.resolution = 'equipment_recovered'
                        AND ",
            $fulfillment,
            ".last_error_code IN ('recovery_equipment_recovered', 'recovery_sale_voided')
                        AND ",
            $obligation,
            ".id IN (closing.equipment_obligation_id, closing.seller_payout_obligation_id)))))"
        )
    };
}
pub(crate) use recovery_closed_fulfillment_sql;

/// 义务 `$obligation` 的原投递 `$delivery` 当前 fence 上已有恢复来源的权威处置（`cod_returned` 或
/// `recovery_equipment_recovered`），且该义务正是处置来源恢复记录的付款或交付义务：随邮件寄出的
/// 装备已由恢复收回，不再是外发在途，去向按恢复记录计。
macro_rules! recovery_disposed_delivery_sql {
    ($obligation:literal, $delivery:literal) => {
        concat!(
            "EXISTS (
                SELECT 1
                FROM public.delivery_dispositions AS reclaimed
                JOIN public.settlement_recoveries AS reclaiming
                  ON reclaiming.id = reclaimed.settlement_recovery_id
                WHERE reclaimed.delivery_id = ",
            $delivery,
            ".id
                  AND reclaimed.fencing_version = ",
            $delivery,
            ".fencing_version
                  AND reclaimed.kind IN ('cod_returned', 'recovery_equipment_recovered')
                  AND ",
            $obligation,
            ".id IN (reclaiming.payment_obligation_id, reclaiming.equipment_obligation_id))"
        )
    };
}
pub(crate) use recovery_disposed_delivery_sql;
