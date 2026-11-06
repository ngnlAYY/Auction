//! 邮件日程的世界相位策略；整条日程平移，唤醒与持久邮箱 due 使用同一计算。

use std::sync::Arc;

use auction_application::scheduling::RecurringSchedule;
use chrono::{DateTime, Duration, Utc};

/// 启动时只计算一次异世界相位；调用方只在隔离开启且同时配置两个世界时调用。
/// 无法计算两个严格递增的未来触发时刻时返回 `None`，由装配根决定启动错误。
pub fn mail_world_phase(schedule: &dyn RecurringSchedule, now: DateTime<Utc>) -> Option<Duration> {
    let first = schedule.next_after(now).filter(|next| *next > now)?;
    let second = schedule.next_after(first).filter(|next| *next > first)?;
    Some((second.signed_duration_since(first) / 2).min(Duration::seconds(150)))
}

/// 平移原日程；不创建任务完成事实，也不改变租约、游标或命令资格。
pub struct PhasedMailSchedule {
    schedule: Arc<dyn RecurringSchedule>,
    phase: Duration,
}

impl PhasedMailSchedule {
    /// 同一世界的收件与维护复用启动时冻结的相位。
    pub fn new(schedule: Arc<dyn RecurringSchedule>, phase: Duration) -> Self {
        Self { schedule, phase }
    }
}

impl RecurringSchedule for PhasedMailSchedule {
    fn next_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        // 先平移查询时刻再平移结果，保留尚未到来的偏移触发点；不能直接给 next(now) 加偏移。
        // @every 的相对间隔在该公式中自然抵消相位，不另改原解析器的节奏语义。
        let next = self
            .schedule
            .next_after(now.checked_sub_signed(self.phase)?)?
            .checked_add_signed(self.phase)?;
        (next > now).then_some(next)
    }
}
