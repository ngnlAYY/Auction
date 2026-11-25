//! isekai 拒收计数窗口：`(t0 - 90 天, t0]`，UTC 左开右闭。

use auction_time::{Duration, Timestamp};

/// isekai 拒收计数窗口的天数（决定 18，固定值）。
pub const ISEKAI_WINDOW_DAYS: i64 = 90;

/// 左开右闭的计数窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsekaiWindow {
    /// 下界，不含。
    pub from_exclusive: Timestamp,
    /// 上界，含；即获证事务的锁后数据库时刻 `t0`。
    pub to_inclusive: Timestamp,
}

impl IsekaiWindow {
    /// 退回时刻是否落在窗内：恰等于下界不计，恰等于上界计。
    pub fn contains(&self, returned_at: Timestamp) -> bool {
        self.from_exclusive < returned_at && returned_at <= self.to_inclusive
    }
}

/// 以获证时刻 `t0` 计算窗口；时刻超出可表示范围时返回 `None`（失败关闭）。
pub fn isekai_window(t0: Timestamp) -> Option<IsekaiWindow> {
    let from_exclusive = t0.checked_sub_signed(Duration::days(ISEKAI_WINDOW_DAYS))?;
    Some(IsekaiWindow { from_exclusive, to_inclusive: t0 })
}
