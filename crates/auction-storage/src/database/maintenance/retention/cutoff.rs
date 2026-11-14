//! 保留期限边界：只读演练与清理共用同一计算。清理批准绑定演练回执，两者以同一快照时间得出
//! 的边界必须完全一致，否则批准的候选集合与实际删除范围会错位。

use chrono::{DateTime, TimeDelta, Utc};

/// 快照时间减去期限；失败只可能来自非法期限，调用方已按政策校验过范围。候选谓词
/// `timestamp <= cutoff` 包含边界本身，边界值由数据库用例验证。
pub(super) fn retention_cutoff(now: DateTime<Utc>, duration_days: i64) -> Option<DateTime<Utc>> {
    if duration_days < 0 {
        return None;
    }
    TimeDelta::try_days(duration_days).and_then(|delta| now.checked_sub_signed(delta))
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Days};

    use super::retention_cutoff;

    #[test]
    fn cutoff_moves_back_exactly_the_policy_duration() {
        let now = DateTime::from_timestamp(10_000_000, 0).expect("valid timestamp");
        assert_eq!(retention_cutoff(now, 0), Some(now));
        assert_eq!(retention_cutoff(now, 7), Some(now - Days::new(7)));
        assert_eq!(retention_cutoff(now, 30), Some(now - Days::new(30)));
        assert_eq!(retention_cutoff(now, 90), Some(now - Days::new(90)));
        assert_eq!(retention_cutoff(now, -1), None);
    }
}
