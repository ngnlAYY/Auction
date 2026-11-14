//! Admin 单一共享身份的全局尝试配额与失败退避；来源地址不参与状态键。

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct AdminLoginDefense {
    limit: u32,
    window: Duration,
    state: Arc<Mutex<LoginWindow>>,
}

struct LoginWindow {
    started: Instant,
    attempts: u32,
    failures: u32,
    last_failure: Option<Instant>,
    blocked_until: Option<Instant>,
}

impl Default for AdminLoginDefense {
    fn default() -> Self {
        Self::new(10, Duration::from_secs(60))
    }
}

impl AdminLoginDefense {
    pub(super) fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            state: Arc::new(Mutex::new(LoginWindow {
                started: Instant::now(),
                attempts: 0,
                failures: 0,
                last_failure: None,
                blocked_until: None,
            })),
        }
    }

    pub(super) fn allow(&self) -> Option<u64> {
        let Ok(mut state) = self.state.lock() else { return Some(1) };
        self.allow_at(&mut state, Instant::now())
    }

    fn allow_at(&self, state: &mut LoginWindow, now: Instant) -> Option<u64> {
        if now.duration_since(state.started) >= self.window {
            state.started = now;
            state.attempts = 0;
        }
        if state.last_failure.is_some_and(|last| now.duration_since(last) >= self.window) {
            state.failures = 0;
            state.blocked_until = None;
        }
        let backoff = state.blocked_until.map(|until| until.saturating_duration_since(now));
        let quota = (state.attempts >= self.limit)
            .then(|| self.window.saturating_sub(now.duration_since(state.started)));
        let remaining = backoff.into_iter().chain(quota).max().unwrap_or_default();
        if !remaining.is_zero() {
            return Some(remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0));
        }
        // 在调用会话端口前领取额度，故并发无效请求也无法绕过共享窗口。
        state.attempts = state.attempts.saturating_add(1);
        None
    }

    pub(super) fn failed(&self) {
        let Ok(mut state) = self.state.lock() else { return };
        let now = Instant::now();
        if self.failed_at(&mut state, now) {
            tracing::warn!(
                target: "auction_security",
                event = "admin_login_failure_threshold",
                failures = state.failures,
                "Admin 登录连续失败，已启用全局退避"
            );
        }
    }

    // 每轮连续失败只在进入退避时告警；不输出 actor、地址或凭据。
    fn failed_at(&self, state: &mut LoginWindow, now: Instant) -> bool {
        if state.last_failure.is_some_and(|last| now.duration_since(last) >= self.window) {
            state.failures = 0;
        }
        state.last_failure = Some(now);
        state.failures = state.failures.saturating_add(1);
        if state.failures >= 5 {
            let seconds = 1_u64 << (state.failures - 5).min(6);
            state.blocked_until = Some(now + Duration::from_secs(seconds.min(60)));
        }
        state.failures == 5
    }

    pub(super) fn succeeded(&self) {
        let Ok(mut state) = self.state.lock() else { return };
        state.failures = 0;
        state.last_failure = None;
        state.blocked_until = None;
        // 成功不能重置尝试配额，否则泄露的 secret 可无限消耗登录存储端口。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_defense_shares_global_attempt_quota() {
        let defense = AdminLoginDefense::new(2, Duration::from_secs(60));
        assert_eq!(defense.allow(), None);
        assert_eq!(defense.allow(), None);
        assert!(defense.allow().is_some());
    }

    #[test]
    fn failures_back_off_exponentially_and_recover_after_quiet_window() {
        let defense = AdminLoginDefense::new(100, Duration::from_secs(60));
        let mut state = defense.state.lock().unwrap();
        let start = state.started;
        for failure in 1..=4 {
            assert!(!defense.failed_at(&mut state, start));
            assert_eq!(state.failures, failure);
            assert_eq!(defense.allow_at(&mut state, start), None);
        }
        assert!(defense.failed_at(&mut state, start));
        assert_eq!(defense.allow_at(&mut state, start), Some(1));
        let next = start + Duration::from_secs(1);
        assert_eq!(defense.allow_at(&mut state, next), None);
        assert!(!defense.failed_at(&mut state, next));
        assert_eq!(defense.allow_at(&mut state, next), Some(2));
        for _ in 0..100 {
            defense.failed_at(&mut state, next);
        }
        assert_eq!(defense.allow_at(&mut state, next), Some(60));
        assert_eq!(defense.allow_at(&mut state, next + Duration::from_secs(60)), None);
        assert_eq!(state.failures, 0);
    }

    #[test]
    fn success_clears_failure_backoff_without_replenishing_attempt_quota() {
        let defense = AdminLoginDefense::new(1, Duration::from_secs(60));
        assert_eq!(defense.allow(), None);
        for _ in 0..5 {
            defense.failed();
        }
        defense.succeeded();
        assert_eq!(defense.state.lock().unwrap().failures, 0);
        assert!(defense.allow().is_some());
    }
}
