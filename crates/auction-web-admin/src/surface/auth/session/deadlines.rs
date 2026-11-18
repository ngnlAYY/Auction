//! 根据服务端剩余时长与单调时钟安排会话复核。

use auction_time::Timestamp;

use crate::api::AdminSession;

const SESSION_CHECK_MARGIN_MS: f64 = 1_000.0;
const MIN_SESSION_CHECK_DELAY_MS: f64 = 2_000.0;

pub(in crate::surface::auth) fn session_lifetime_millis(session: &AdminSession) -> i64 {
    session_deadline(session).signed_duration_since(session.server_time).num_milliseconds()
}

fn session_deadline(session: &AdminSession) -> Timestamp {
    session.expires_at.min(session.absolute_expires_at)
}

pub(super) fn is_newer_projection(candidate: &AdminSession, current: &AdminSession) -> bool {
    // 同代截止不得倒退。截止相同时用较晚的服务端观察时刻校准，整份投影保留接收基准。
    (session_deadline(candidate), candidate.server_time)
        > (session_deadline(current), current.server_time)
}

pub(super) fn session_check_delay(
    lifetime_millis: i64,
    received_at_millis: f64,
    now_millis: f64,
) -> u32 {
    let remaining = lifetime_millis as f64 - (now_millis - received_at_millis).max(0.0);
    let before_deadline = remaining - SESSION_CHECK_MARGIN_MS;
    // 截止将至时等到截止之后，避免成功 GET 返回短剩余时长后再立即查一次。
    let delay = if before_deadline >= MIN_SESSION_CHECK_DELAY_MS {
        before_deadline
    } else {
        (remaining + SESSION_CHECK_MARGIN_MS).max(MIN_SESSION_CHECK_DELAY_MS)
    };
    delay.ceil().min(f64::from(i32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};

    use super::{session_check_delay, session_lifetime_millis};
    use crate::api::AdminSession;

    fn session(actor: &str, csrf_token: &str) -> AdminSession {
        let server_time = Timestamp::parse_from_rfc3339("2026-11-14T00:00:00Z").unwrap();
        AdminSession {
            actor: actor.to_owned(),
            csrf_token: csrf_token.to_owned(),
            expires_at: server_time.checked_add_signed(Duration::minutes(15)).unwrap(),
            absolute_expires_at: server_time.checked_add_signed(Duration::hours(8)).unwrap(),
            server_time,
            received_at_millis: 0.0,
        }
    }

    #[test]
    fn session_checks_use_server_duration_and_elapsed_monotonic_time() {
        let baseline = session("ops", "csrf-a");
        let lifetime = session_lifetime_millis(&baseline);
        assert_eq!(lifetime, 900_000);
        // 单调时钟的原点可比服务端 UTC 时刻大或小，间隔只取其增量。
        for received in [1.0, baseline.server_time.timestamp_millis() as f64 + 86_400_000.0] {
            assert_eq!(session_check_delay(lifetime, received, received + 250.0), 898_750);
        }
    }

    #[test]
    fn short_or_elapsed_session_deadlines_wait_past_cutoff() {
        let mut current = session("ops", "csrf-a");
        current.absolute_expires_at =
            current.server_time.checked_add_signed(Duration::seconds(3)).unwrap();
        let lifetime = session_lifetime_millis(&current);
        assert_eq!(lifetime, 3_000);
        assert_eq!(session_check_delay(lifetime, 100.0, 100.0), 2_000);
        assert_eq!(session_check_delay(lifetime, 100.0, 2_100.0), 2_000);
        for remaining in [0, -50] {
            assert_eq!(session_check_delay(remaining, 100.0, 100.0), 2_000);
        }
    }
}
