//! 保留期限边界计算与纯分类；与 SQL 谓词一一对应，便于在没有数据库时验证边界。

use chrono::{DateTime, TimeDelta, Utc};

/// 快照时间减去期限；失败只可能来自非法期限，调用方已按政策校验过范围。
pub(crate) fn retention_cutoff(now: DateTime<Utc>, duration_days: i64) -> Option<DateTime<Utc>> {
    if duration_days < 0 {
        return None;
    }
    TimeDelta::try_days(duration_days).and_then(|delta| now.checked_sub_signed(delta))
}

/// 时间戳边界：恰好等于 cutoff 视为已过保持期（与 SQL `timestamp <= cutoff` 一致）。
#[cfg(test)]
pub(crate) fn age_candidate(timestamp: DateTime<Utc>, cutoff: DateTime<Utc>) -> bool {
    timestamp <= cutoff
}

/// 会话分类；只有 `Revoked` 与 `Expired` 是删除候选。
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionRetentionClass {
    /// 已注销：按决策 29 立即成为候选，不含额外界限。
    Revoked,
    /// 截止时间已过且超过 30 天宽限。
    Expired,
    /// 截止时间已过但仍在宽限期内；保留。
    Grace,
    /// 尚未截止的有限会话；保留。
    Active,
    /// 两个截止均为 NULL 的永久 Public 会话；未注销时永不删除。
    PermanentActive,
}

#[cfg(test)]
pub(crate) fn classify_session(
    revoked_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> SessionRetentionClass {
    if revoked_at.is_some() {
        return SessionRetentionClass::Revoked;
    }
    match expires_at {
        None => SessionRetentionClass::PermanentActive,
        Some(deadline) if deadline <= cutoff => SessionRetentionClass::Expired,
        Some(deadline) if deadline <= now => SessionRetentionClass::Grace,
        Some(_) => SessionRetentionClass::Active,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Days, TimeDelta, Utc};

    use super::{SessionRetentionClass, age_candidate, classify_session, retention_cutoff};

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("valid timestamp")
    }

    fn now() -> DateTime<Utc> {
        at(10_000_000)
    }

    #[test]
    fn cutoff_moves_back_exactly_the_policy_duration() {
        let now = now();
        assert_eq!(retention_cutoff(now, 0), Some(now));
        assert_eq!(retention_cutoff(now, 30), Some(now - Days::new(30)));
        assert_eq!(retention_cutoff(now, 90), Some(now - Days::new(90)));
        assert_eq!(retention_cutoff(now, 7), Some(now - Days::new(7)));
        assert_eq!(retention_cutoff(now, -1), None);
    }

    #[test]
    fn age_boundary_is_inclusive_at_cutoff_only() {
        let cutoff = retention_cutoff(now(), 30).expect("cutoff");
        assert!(age_candidate(cutoff, cutoff));
        assert!(!age_candidate(cutoff + TimeDelta::nanoseconds(1), cutoff));
        assert!(age_candidate(cutoff - Days::new(1), cutoff));
    }

    #[test]
    fn revoked_sessions_are_candidates_regardless_of_deadline() {
        let now = now();
        let cutoff = retention_cutoff(now, 30).expect("cutoff");
        assert_eq!(
            classify_session(Some(now), Some(now + Days::new(10)), cutoff, now),
            SessionRetentionClass::Revoked
        );
        assert_eq!(classify_session(Some(now), None, cutoff, now), SessionRetentionClass::Revoked);
    }

    #[test]
    fn expired_session_boundary_includes_exact_cutoff() {
        let now = now();
        let cutoff = retention_cutoff(now, 30).expect("cutoff");
        assert_eq!(
            classify_session(None, Some(cutoff), cutoff, now),
            SessionRetentionClass::Expired
        );
        assert_eq!(
            classify_session(None, Some(cutoff + TimeDelta::nanoseconds(1)), cutoff, now),
            SessionRetentionClass::Grace
        );
    }

    #[test]
    fn active_and_permanent_sessions_are_never_candidates() {
        let now = now();
        let cutoff = retention_cutoff(now, 30).expect("cutoff");
        assert_eq!(
            classify_session(None, None, cutoff, now),
            SessionRetentionClass::PermanentActive
        );
        assert_eq!(
            classify_session(None, Some(now + TimeDelta::nanoseconds(1)), cutoff, now),
            SessionRetentionClass::Active
        );
        assert_eq!(classify_session(None, Some(now), cutoff, now), SessionRetentionClass::Grace);
    }
}
