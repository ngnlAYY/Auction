//! operations alert 来源、signal 类型和安全错误。
//! 子模块：`source`（来源闭集与处置手册锚点）、`kind`（种类闭集与分级）、`severity`（分级闭集与
//! 每日摘要节奏）、`thresholds`（固定观察窗口与年龄水位判定）、`errors`（安全错误）、`signal`
//! （fingerprint、边界校验与通知摘要）。

mod errors;
mod kind;
mod severity;
mod signal;
mod source;
mod thresholds;

pub use errors::OperationsAlertError;
pub use kind::OperationsAlertKind;
pub use severity::{OPERATIONS_ALERT_DIGEST_INTERVAL, OperationsAlertSeverity};
pub use signal::OperationsAlertSignal;
pub use source::OperationsAlertSource;
pub(super) use thresholds::reached;
pub use thresholds::{
    OPERATIONS_ALERT_ACCOUNT_BUSY_AGE, OPERATIONS_ALERT_LIFECYCLE_GRACE,
    OPERATIONS_ALERT_MAIL_POLL_STALL_AGE, OPERATIONS_ALERT_QUEUE_LATENCY_AGE,
    OPERATIONS_ALERT_RECONCILIATION_STALE_AGE, OPERATIONS_ALERT_RESTORE_DRILL_MAX_AGE,
    OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE,
};
