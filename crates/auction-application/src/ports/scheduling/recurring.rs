use chrono::{DateTime, Utc};

/// Worker 使用的可重复调度窄端口。
///
/// 配置适配器负责解析 cron、时区或固定间隔；任务编排只读取下一次 UTC 触发时刻，不能把
/// 内存定时器当成持久化任务 due、游标或租约的权威来源。
pub trait RecurringSchedule: Send + Sync {
    /// 返回严格晚于 `now` 的下一次触发时刻；无合法未来时刻时失败关闭。
    fn next_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>>;
}
