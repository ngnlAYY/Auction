//! Item Shop 余额的安全快照模型。

use crate::clients::mooglemail::MoogleMailWorld;
use chrono::{DateTime, Utc};

/// 一次同步读取的 world wallet Credits 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailBalanceSnapshot {
    pub world: MoogleMailWorld,
    /// 页面与脚本均证明的 Credits 余额。
    pub credits: i64,
    pub observed_at: DateTime<Utc>,
}

impl MoogleMailBalanceSnapshot {
    pub fn validate(&self) -> bool {
        self.credits >= 0 && self.observed_at.timestamp_nanos_opt().is_some()
    }
}
