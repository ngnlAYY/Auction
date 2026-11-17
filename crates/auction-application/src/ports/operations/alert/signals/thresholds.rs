//! 告警来源的固定最小观察窗口与年龄水位判定。

use std::time::Duration;

/// 新增来源的固定最小观察窗口。它们不进入运行配置：阈值只决定“何时算异常”，
/// 放宽或收紧都需要新的代码审阅，避免运维配置扩大一遍扫描的告警面。
/// 收件轮询停滞：最后一次成功轮询水位超过该年龄即告警。
pub const OPERATIONS_ALERT_MAIL_POLL_STALL_AGE: Duration = Duration::from_secs(15 * 60);
/// 生命周期逾期宽限：场次结束或慢速代理到期后仍允许 Worker 推进的窗口。
pub const OPERATIONS_ALERT_LIFECYCLE_GRACE: Duration = Duration::from_secs(15 * 60);
/// 队列延迟：已被领取但超过该年龄仍未推进的投递。
pub const OPERATIONS_ALERT_QUEUE_LATENCY_AGE: Duration = Duration::from_secs(10 * 60);
/// 外部账号持续忙：同一世界余额读取连续处于忙状态的持续时间。
pub const OPERATIONS_ALERT_ACCOUNT_BUSY_AGE: Duration = Duration::from_secs(15 * 60);
/// WAL 暂存积压：`archive_timeout` 为 300 秒、上传器每 5 分钟运行一次，因此一段暂存
/// 超过 30 分钟仍未上传说明归档或上传链路已持续失败。该阈值不进入运行配置。
pub const OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE: Duration = Duration::from_secs(30 * 60);
/// 恢复演练每月一次；最近一次成功演练超过 35 天（月度加 5 天余量）即逾期。
/// 该阈值不进入运行配置。
pub const OPERATIONS_ALERT_RESTORE_DRILL_MAX_AGE: Duration = Duration::from_secs(35 * 24 * 3_600);
/// 资产对账报告过期：固定对账周期为 6 小时，报告超过 24 小时未更新说明对账视野已失效。
pub const OPERATIONS_ALERT_RECONCILIATION_STALE_AGE: Duration = Duration::from_secs(24 * 3_600);

/// 年龄水位是否达到固定阈值；负值（时钟回拨归一化）不告警。
pub(in crate::ports::operations::alert) const fn reached(
    age_seconds: i64,
    threshold: Duration,
) -> bool {
    age_seconds >= 0 && age_seconds >= threshold.as_secs() as i64
}
