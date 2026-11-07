//! 同一账号外部请求共用的容量为 1 分级请求门：业务写入优先，只读轮询按保底获得份额；无突发。
//! 状态机在 `auction_application::mail::PriorityGateState`，这里只负责登记、唤醒与取消。
//!
//! MoogleMail 与论坛客户端各自按账号持有一个门：同一远端账号的物理请求仍然串行，
//! 优先级只决定先后，不改变总速率、结果分类或写入未知语义。

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use auction_application::mail::{
    GateClass, GatePoll, GateTicket, LOW_PRIORITY_FLOOR_AFTER, PriorityGateState,
};
use tokio::time;

/// 同一账号所有物理请求共享一个门；克隆客户端共享同一份状态。
pub(crate) struct PriorityGate {
    state: Mutex<PriorityGateState>,
    interval: Duration,
}

impl PriorityGate {
    /// `interval` 为两次获准请求的最小间隔，必须为正。
    pub(crate) fn new(interval: Duration) -> Self {
        debug_assert!(!interval.is_zero(), "request interval must be positive");
        Self {
            state: Mutex::new(PriorityGateState::new(
                Instant::now(),
                interval,
                LOW_PRIORITY_FLOOR_AFTER,
            )),
            interval,
        }
    }

    pub(crate) const fn interval(&self) -> Duration {
        self.interval
    }

    /// 等待一个令牌；返回 `Err` 只在票据被取消等内部不一致时出现，调用方必须放弃本次请求。
    pub(crate) async fn wait(&self, class: GateClass) -> Result<(), ()> {
        let ticket = self.with_state(|state| state.register(class));
        // 等待者被丢弃（预算取消、上层超时）时同步撤销票据，不能占用后续令牌。
        let mut registration = Registration { gate: self, ticket, armed: true };
        loop {
            match self.with_state(|state| state.poll(ticket, Instant::now())) {
                GatePoll::Granted => {
                    registration.armed = false;
                    return Ok(());
                }
                GatePoll::WaitUntil(at) => {
                    time::sleep(at.saturating_duration_since(Instant::now())).await;
                }
                GatePoll::Expired => return Err(()),
            }
        }
    }

    fn with_state<T>(&self, run: impl FnOnce(&mut PriorityGateState) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(|poison| poison.into_inner());
        run(&mut state)
    }
}

struct Registration<'a> {
    gate: &'a PriorityGate,
    ticket: GateTicket,
    armed: bool,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.gate.with_state(|state| state.cancel(self.ticket));
        }
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::clients::gate as parent;
    use std::sync::Arc;

    #[tokio::test]
    async fn gate_spaces_requests_without_burst() {
        let interval = Duration::from_millis(40);
        let gate = PriorityGate::new(interval);
        gate.wait(GateClass::Foreground).await.expect("first slot is immediate");
        let started = Instant::now();
        gate.wait(GateClass::Foreground).await.expect("second slot granted");
        assert!(started.elapsed() >= interval.saturating_sub(Duration::from_millis(5)));
    }

    #[tokio::test]
    async fn cancelled_waiter_releases_its_ticket() {
        let interval = Duration::from_millis(80);
        let gate = Arc::new(PriorityGate::new(interval));
        gate.wait(GateClass::Foreground).await.expect("first slot is immediate");
        // 在第一个令牌后登记并立即取消，模拟上层超时丢弃等待者。
        let cancelled_gate = Arc::clone(&gate);
        let cancelled =
            tokio::spawn(async move { cancelled_gate.wait(GateClass::Background).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancelled.abort();
        let _ = cancelled.await;
        let started = Instant::now();
        gate.wait(GateClass::Foreground).await.expect("slot after cancellation");
        // 取消的票据不能额外占用一个完整间隔。
        assert!(started.elapsed() < interval * 3);
    }

    #[test]
    fn foreground_registration_precedes_queued_background_work() {
        let gate = PriorityGate::new(Duration::from_millis(50));
        let now = Instant::now();
        // 后登记的前台写入排在先登记的后台只读请求之前，但仍不越过当前时间槽。
        let background = gate.with_state(|state| state.register(GateClass::Background));
        let foreground = gate.with_state(|state| state.register(GateClass::Foreground));
        assert_eq!(gate.with_state(|state| state.poll(foreground, now)), GatePoll::Granted);
        assert_eq!(
            gate.with_state(|state| state.poll(background, now)),
            GatePoll::WaitUntil(now + gate.interval())
        );
        assert_eq!(
            gate.with_state(|state| state.poll(background, now + gate.interval())),
            GatePoll::Granted
        );
    }
}
