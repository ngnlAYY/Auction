//! “已寄出、等待领取”的共享 SQL 判定。
//!
//! Send 履约寄出后，系统只在等收件人在游戏内领取附件；这一步不再需要 Worker 寄送，资产也已
//! 离开 Bot（货款记为 `spent`、装备在邮件里）。告警看板的积压分类与资产对账的在途分类都按
//! 同一条件区分“尚未寄出”与“等待领取”，避免各查询各自复制后漂移。判定只用于展示与报告，
//! 不授权任何资金或资产动作，也不改变义务状态。

/// 履约执行态为 `enqueued`、操作为 `send` 且其投递已 `succeeded`。`$fulfillment` 与 `$delivery`
/// 分别是 `settlement_obligation_fulfillments` 与 `deliveries` 的别名；经 LEFT JOIN 缺行时整体为
/// NULL，调用方必须用 `COALESCE(…, false)` 收口，不能把未知当作已寄出。
macro_rules! sent_awaiting_collection_sql {
    ($fulfillment:literal, $delivery:literal) => {
        concat!(
            "(",
            $fulfillment,
            ".execution_state = 'enqueued' AND ",
            $fulfillment,
            ".delivery_operation = 'send' AND ",
            $delivery,
            ".status = 'succeeded')"
        )
    };
}
pub(crate) use sent_awaiting_collection_sql;
