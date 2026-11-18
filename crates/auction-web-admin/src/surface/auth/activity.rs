//! 按人工操作合并 Admin 会话续期请求。

use super::session::session_lifetime_millis;
use crate::api::AdminSession;

pub(super) const LOGOUT_BUTTON_CLASS: &str = "admin-shell__logout";
const MAX_RENEWAL_INTERVAL_MS: f64 = 60_000.0;
const MIN_RENEWAL_INTERVAL_MS: f64 = 5_000.0;

/// 最近一次登录或续期确定的合并间隔与单调时钟基准。
#[derive(Clone, Copy, PartialEq)]
pub(super) struct RenewalTiming {
    since_millis: f64,
    pub(super) interval_millis: f64,
}

impl RenewalTiming {
    pub(super) fn from_session(session: &AdminSession) -> Self {
        Self {
            since_millis: session.received_at_millis,
            interval_millis: renewal_interval_millis(session_lifetime_millis(session)),
        }
    }
}

fn renewal_interval_millis(lifetime_millis: i64) -> f64 {
    (lifetime_millis as f64 / 3.0).clamp(MIN_RENEWAL_INTERVAL_MS, MAX_RENEWAL_INTERVAL_MS)
}

pub(super) fn estimated_expiration_millis(session: &AdminSession) -> f64 {
    session.received_at_millis + session_lifetime_millis(session) as f64
}

pub(super) fn activity_delay(
    timing: Option<RenewalTiming>,
    latest_input: Option<f64>,
    now: f64,
    expires_at: f64,
    retry_not_before: f64,
) -> Option<u32> {
    let latest_input = latest_input?;
    let deferred_by_rate_limit = latest_input < retry_not_before;
    let scheduled = if deferred_by_rate_limit {
        retry_not_before
    } else {
        timing.map_or(now, |timing| timing.since_millis + timing.interval_millis)
    };
    let delay = (scheduled - now).max(0.0).ceil();
    let due_at = now + delay;
    // 限流可能要求等待超过 60 秒。已排队的新输入只要赶得上会话截止就仍可派发。
    (due_at < expires_at
        && (deferred_by_rate_limit || due_at - latest_input <= MAX_RENEWAL_INTERVAL_MS))
        .then_some(delay as u32)
}

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};

    use super::{
        RenewalTiming, activity_delay, estimated_expiration_millis, renewal_interval_millis,
    };
    use crate::api::AdminSession;

    fn session(idle_seconds: i64, absolute_seconds: i64) -> AdminSession {
        let server_time = Timestamp::parse_from_rfc3339("2026-11-18T00:00:00Z").unwrap();
        AdminSession {
            actor: "operator".to_owned(),
            csrf_token: "test-csrf".to_owned(),
            expires_at: server_time.checked_add_signed(Duration::seconds(idle_seconds)).unwrap(),
            absolute_expires_at: server_time
                .checked_add_signed(Duration::seconds(absolute_seconds))
                .unwrap(),
            server_time,
            received_at_millis: 100.0,
        }
    }

    #[test]
    fn interval_uses_one_third_of_the_earliest_deadline_with_bounds() {
        assert_eq!(renewal_interval_millis(60_000), 20_000.0);
        assert_eq!(renewal_interval_millis(900_000), 60_000.0);
        assert_eq!(RenewalTiming::from_session(&session(900, 30)).interval_millis, 10_000.0);
        assert_eq!(renewal_interval_millis(6_000), 5_000.0);
    }

    #[test]
    fn one_early_input_schedules_a_renewal_before_each_deadline() {
        for (idle, absolute, interval) in
            [(60, 3_600, 20_000), (900, 3_600, 60_000), (900, 30, 10_000)]
        {
            let session = session(idle, absolute);
            let timing = RenewalTiming::from_session(&session);
            let expires_at = estimated_expiration_millis(&session);
            let input = session.received_at_millis + 1_000.0;
            let delay = activity_delay(Some(timing), Some(input), input, expires_at, 0.0).unwrap();
            assert_eq!(delay, interval - 1_000);
            let due_at = input + f64::from(delay);
            assert!(due_at < expires_at);
            assert_eq!(activity_delay(Some(timing), Some(input), due_at, expires_at, 0.0), Some(0));
            assert_eq!(activity_delay(Some(timing), None, due_at, expires_at, 0.0), None);
            assert_eq!(
                activity_delay(Some(timing), Some(input), expires_at, expires_at, 0.0),
                None
            );
        }
    }

    #[test]
    fn interval_floor_cannot_queue_a_renewal_at_the_deadline() {
        let session = session(900, 5);
        let timing = RenewalTiming::from_session(&session);
        let expires_at = estimated_expiration_millis(&session);
        let input = session.received_at_millis + 1_000.0;
        assert_eq!(activity_delay(Some(timing), Some(input), input, expires_at, 0.0), None);
    }

    #[test]
    fn continuous_input_keeps_one_renewal_at_each_minute_boundary() {
        let timing = Some(RenewalTiming { since_millis: 1_000.0, interval_millis: 60_000.0 });
        let expires_at = 900_000.0;
        assert_eq!(activity_delay(timing, Some(2_000.0), 2_000.0, expires_at, 0.0), Some(59_000));
        assert_eq!(activity_delay(timing, Some(35_000.0), 35_000.0, expires_at, 0.0), Some(26_000));
        assert_eq!(activity_delay(timing, Some(60_000.0), 61_000.0, expires_at, 0.0), Some(0));
        let renewed = Some(RenewalTiming { since_millis: 61_000.0, interval_millis: 60_000.0 });
        assert_eq!(
            activity_delay(renewed, Some(62_000.0), 62_000.0, expires_at, 0.0),
            Some(59_000)
        );
    }

    #[test]
    fn activity_can_renew_once_then_idle_and_stale_input_cannot_renew() {
        // 冷启读取既有 Cookie 会话时，首个输入可以立即续期。
        let expires_at = 900_000.0;
        let timing = Some(RenewalTiming { since_millis: 0.0, interval_millis: 60_000.0 });
        assert_eq!(activity_delay(None, Some(100.0), 100.0, expires_at, 0.0), Some(0));
        assert_eq!(activity_delay(None, None, 100.0, expires_at, 0.0), None);
        assert_eq!(activity_delay(timing, Some(75_000.0), 75_000.0, expires_at, 0.0), Some(0));
        assert_eq!(activity_delay(timing, None, 135_000.0, expires_at, 0.0), None);
        assert_eq!(activity_delay(timing, Some(500.0), 65_000.0, expires_at, 0.0), None);
    }

    #[test]
    fn rate_limited_activity_waits_for_retry_after_and_new_input() {
        let timing = Some(RenewalTiming { since_millis: 1_000.0, interval_millis: 5_000.0 });
        assert_eq!(activity_delay(timing, None, 9_000.0, 90_000.0, 12_000.0), None);
        assert_eq!(activity_delay(timing, Some(9_000.0), 9_000.0, 90_000.0, 12_000.0), Some(3_000));
        assert_eq!(activity_delay(timing, Some(9_000.0), 12_000.0, 90_000.0, 12_000.0), Some(0));
        assert_eq!(activity_delay(timing, Some(9_000.0), 9_000.0, 12_000.0, 12_000.0), None);
    }
}
