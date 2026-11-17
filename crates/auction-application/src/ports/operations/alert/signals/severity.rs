//! 告警分级闭集与非紧急（每日摘要）信号的重复通知节奏。

use std::fmt;
use std::time::Duration;

use serde::Serialize;

/// 非紧急（每日摘要）信号的重复通知节奏。
pub const OPERATIONS_ALERT_DIGEST_INTERVAL: Duration = Duration::from_secs(24 * 3_600);

/// 告警分级闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OperationsAlertSeverity {
    /// 需要立即处理；按策略冷却重复通知。
    Urgent,
    /// 非紧急；重复通知并入每日摘要节奏（摘要聚合属于告警台）。
    Digest,
}

impl OperationsAlertSeverity {
    /// 返回日志与告警台共用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::Digest => "digest",
        }
    }
}

impl fmt::Display for OperationsAlertSeverity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
