//! “已寄出”的共享 SQL 判定：Send 投递成功寄出的持久事实，以及其上的“等待领取”与“已寄出、
//! 尚未满足”。
//!
//! Send 履约寄出后，资产已随邮件离开 Bot（货款记为 `spent`、装备在邮件里），系统只等收件人在
//! 游戏内领取。寄出是不可撤回的持久事实：之后履约停在 `enqueued`（等待领取），或因领取证据不符、
//! 结果未知转为 `manual_reconcile`/`failed`，都只说明领取证据如何，不改变资产位置。告警看板的
//! 积压分类只拆出“等待领取”；资产对账按寄出事实把两者都列为在途，后者单列为在途待核对。各查询
//! 共用这里的判定，避免各自复制后漂移。判定只用于展示与报告，不授权任何资金或资产动作，也不
//! 改变义务状态。
//!
//! 三个寄出宏的 `$fulfillment` 与 `$delivery` 分别是 `settlement_obligation_fulfillments` 与
//! `deliveries` 的别名；经 LEFT JOIN 缺行时表达式整体为 NULL，调用方必须用 `COALESCE(…, false)`
//! 收口，不能把未知当作已寄出。原投递的系统退件已被恢复流程消费时，邮件已不在收件人邮箱，看板与
//! 告警的“等待领取”另按 `recovery_consumed_return_sql!` 排除（资产对账的在途仍按寄出事实与观察）。

/// 履约操作为 `send` 且其投递已 `succeeded`：成功寄出的持久事实（不含括号，只供本模块组合）。
macro_rules! sent_delivery_sql {
    ($fulfillment:literal, $delivery:literal) => {
        concat!(
            $fulfillment,
            ".delivery_operation = 'send' AND ",
            $delivery,
            ".status = 'succeeded'"
        )
    };
}
pub(crate) use sent_delivery_sql;

/// 已寄出、等待领取：成功寄出后履约仍为 `enqueued`。看板与积压告警的“等待领取”口径。
macro_rules! sent_awaiting_collection_sql {
    ($fulfillment:literal, $delivery:literal) => {
        concat!(
            "(",
            $fulfillment,
            ".execution_state = 'enqueued' AND ",
            $crate::database::settlement::fulfillment::sent_delivery_sql!($fulfillment, $delivery),
            ")"
        )
    };
}
pub(crate) use sent_awaiting_collection_sql;

/// 恢复流程已消费义务 `$obligation`（`settlement_obligations` 别名）原投递本轮的系统退件：义务是某条
/// 恢复记录的付款或交付义务，恢复记录当前退件指向一封已被它消费、来源为该义务的退件。投递仍是
/// `succeeded`、履约仍是 `enqueued`，但邮件已退回 Bot，收件人不能再领取；之后的领回、Admin 确认与
/// 重寄都由恢复流程承担，看板与告警不再按等待领取或积压计。恢复流程重寄（main COD 未达上限）时
/// 清空当前退件指针，新一轮发送重新按原口径计。消费事实只追加，指针与消费在同一事务写入。
macro_rules! recovery_consumed_return_sql {
    ($obligation:literal) => {
        concat!(
            "EXISTS (
                SELECT 1
                FROM public.settlement_recoveries AS reclaiming
                JOIN public.settlement_recovery_return_consumptions AS consumed
                  ON consumed.recovery_id = reclaiming.id
                 AND consumed.return_mail_id = reclaiming.return_mail_id
                JOIN public.inbound_mail AS returned_mail
                  ON returned_mail.id = reclaiming.return_mail_id
                WHERE ",
            $obligation,
            ".id IN (reclaiming.payment_obligation_id, reclaiming.equipment_obligation_id)
                  AND returned_mail.decision_metadata ->> 'obligation_id' = ",
            $obligation,
            ".id::text)"
        )
    };
}
pub(crate) use recovery_consumed_return_sql;

/// 已寄出、尚未满足：成功寄出后履约仍在等待领取，或已转 `manual_reconcile`/`failed` 待核对。
///
/// 资产对账的在途只认这条持久寄出事实，不随可变的执行态把已寄出的资产重新计入待付或应在
/// Armory；其中不满足 `sent_awaiting_collection_sql!` 的即在途待核对。执行态只列出寄出后可能
/// 出现的三种，其余组合（含 `satisfied`）不算在途，按原口径失败关闭。
macro_rules! sent_unsatisfied_sql {
    ($fulfillment:literal, $delivery:literal) => {
        concat!(
            "(",
            $fulfillment,
            ".execution_state IN ('enqueued', 'failed', 'manual_reconcile') AND ",
            $crate::database::settlement::fulfillment::sent_delivery_sql!($fulfillment, $delivery),
            ")"
        )
    };
}
pub(crate) use sent_unsatisfied_sql;
