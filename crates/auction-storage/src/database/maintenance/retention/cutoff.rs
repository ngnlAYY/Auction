//! 保留期限边界：只读演练与清理共用同一计算。清理批准绑定演练回执，两者以同一快照时间得出
//! 的边界必须完全一致，否则批准的候选集合与实际删除范围会错位。

use auction_time::{TimeDelta, Timestamp};

/// 快照时间减去期限；失败只可能来自非法期限，调用方已按政策校验过范围。候选谓词
/// `timestamp <= cutoff` 包含边界本身，边界值由数据库用例验证。
pub(super) fn retention_cutoff(now: Timestamp, duration_days: i64) -> Option<Timestamp> {
    if duration_days < 0 {
        return None;
    }
    TimeDelta::try_days(duration_days).and_then(|delta| now.checked_sub_signed(delta))
}

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};

    use super::retention_cutoff;

    #[test]
    fn cutoff_moves_back_exactly_the_policy_duration() {
        let now = Timestamp::from_timestamp(10_000_000, 0).expect("valid timestamp");
        assert_eq!(retention_cutoff(now, 0), Some(now));
        assert_eq!(retention_cutoff(now, 7), Some(now - Duration::days(7)));
        assert_eq!(retention_cutoff(now, 30), Some(now - Duration::days(30)));
        assert_eq!(retention_cutoff(now, 90), Some(now - Duration::days(90)));
        assert_eq!(retention_cutoff(now, -1), None);
    }
}
