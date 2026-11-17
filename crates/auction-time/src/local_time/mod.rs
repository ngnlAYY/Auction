//! 本地墙上时间到 UTC 时刻的换算。
//!
//! 时区数据库按本地时间查找时，会在 UTC 偏移不变的规则切换处多报一个并不存在的重复时刻
//! （例如 `America/Nuuk` 的 2023-10-28 23:00–23:59）。这里只承认换算回去偏移仍然相符的候选，
//! 排期与 cron 都经这一个入口解析墙上时间。

#[cfg(test)]
mod tests;

use jiff::civil;
use jiff::tz::AmbiguousOffset;
use jiff::tz::Offset;
use jiff::tz::TimeZone;

use crate::instant::Timestamp;

/// 某时区下一个本地墙上时间对应的时刻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTime {
    /// 唯一对应一个时刻。
    Unique(Timestamp),
    /// 时钟回拨使这个墙上时间出现两次。
    Repeated { earlier: Timestamp, later: Timestamp },
    /// 时钟前跳使这个墙上时间不存在；`after` 是跳时之后生效的 UTC 偏移。
    Skipped { after: Offset },
}

impl LocalTime {
    /// 解析 `local` 在 `timezone` 下对应的时刻。
    pub fn resolve(timezone: &TimeZone, local: civil::DateTime) -> Self {
        let (before, after) = match timezone.to_ambiguous_timestamp(local).offset() {
            AmbiguousOffset::Unambiguous { offset } => (offset, offset),
            AmbiguousOffset::Fold { before, after } | AmbiguousOffset::Gap { before, after } => {
                (before, after)
            }
        };
        let verified = |offset: Offset| {
            let instant = offset.to_timestamp(local).ok()?;
            (timezone.to_offset(instant) == offset).then_some(Timestamp::from(instant))
        };
        match (verified(before), verified(after)) {
            (Some(first), Some(second)) if first != second => {
                Self::Repeated { earlier: first.min(second), later: first.max(second) }
            }
            (Some(instant), _) | (None, Some(instant)) => Self::Unique(instant),
            (None, None) => Self::Skipped { after },
        }
    }
}
