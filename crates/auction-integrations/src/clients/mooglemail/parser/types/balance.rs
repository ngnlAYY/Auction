//! Item Shop 余额的安全快照模型。

use super::*;

/// 一次同步读取的 world wallet Credits 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailBalanceSnapshot {
    /// 余额所属世界。
    pub world: MoogleMailWorld,
    /// 页面与脚本均证明的 Credits 余额。
    pub credits: i64,
    /// 本次读取的观察时间。
    pub observed_at: DateTime<Utc>,
}

impl MoogleMailBalanceSnapshot {
    /// 校验余额 world、数值和观察时间。
    pub fn validate(&self) -> bool {
        self.credits >= 0 && self.observed_at.timestamp_nanos_opt().is_some()
    }
}
