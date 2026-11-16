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
//! 三个宏的 `$fulfillment` 与 `$delivery` 分别是 `settlement_obligation_fulfillments` 与
//! `deliveries` 的别名；经 LEFT JOIN 缺行时表达式整体为 NULL，调用方必须用 `COALESCE(…, false)`
//! 收口，不能把未知当作已寄出。

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
