//! 请求门的纯状态机：容量 1 的令牌桶、两档优先级与低优先级保底。只决定先后，不决定业务
//! 结果、证明、租约或重试；持久 outbox 与 sending 围栏仍是唯一权威。
//!
//! 投递的出队优先级由领取 SQL 按 `public.delivery_send_category` 排序决定，不在这里建模。
//! 只读轮询与维护属于请求门的最低档，保底规则只保证它们不会在持续写入下被无限推迟，
//! 不提高总速率。

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// 请求门的两档：业务写入优先于只读轮询；保底规则只作用于最低档。
/// 在途锁与操作锁按同一档位排队。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateClass {
    /// compose、资产动作与最终发送等业务写入。
    Foreground,
    /// 收件箱/已读列表、Sent 扫描、余额与维护等只读请求。
    Background,
}

/// 连续前台授予达到该值后，下一个令牌必须给排队的后台请求。
pub(super) const LOW_PRIORITY_FLOOR_AFTER: u32 = 3;

/// 已登记的令牌请求；只在本进程内有效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GateTicket(u64);

/// 单个等待者对令牌的观察结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GatePoll {
    /// 已获得唯一令牌；调用方可以继续发出一次请求。
    Granted,
    /// 尚未轮到；在该时刻之前不应再次忙轮询。
    WaitUntil(Instant),
    /// 票据已被取消（等待者被丢弃）；调用方必须放弃本次请求。
    Expired,
}

/// 容量为 1 的分级令牌桶状态机：无突发、总量不超过配置间隔、后台请求按保底获得份额。
#[derive(Debug)]
pub(super) struct PriorityGateState {
    interval: Duration,
    floor_after: u32,
    next_free: Instant,
    foreground_streak: u32,
    next_ticket: u64,
    queue: VecDeque<(GateTicket, GateClass)>,
}

impl PriorityGateState {
    /// `interval` 必须为正；`floor_after` 为 0 表示不设保底（不推荐）。
    pub(super) fn new(now: Instant, interval: Duration, floor_after: u32) -> Self {
        Self {
            interval,
            floor_after,
            next_free: now,
            foreground_streak: 0,
            next_ticket: 0,
            queue: VecDeque::new(),
        }
    }

    /// 登记一个等待者；前台排在所有前台之后，仍先于后台。
    pub(super) fn register(&mut self, class: GateClass) -> GateTicket {
        let ticket = loop {
            let candidate = GateTicket(self.next_ticket);
            self.next_ticket = self.next_ticket.wrapping_add(1);
            if !self.is_registered(candidate) {
                break candidate;
            }
        };
        match class {
            GateClass::Foreground => {
                let index = self
                    .queue
                    .iter()
                    .position(|(_, queued)| *queued == GateClass::Background)
                    .unwrap_or(self.queue.len());
                self.queue.insert(index, (ticket, class));
            }
            GateClass::Background => self.queue.push_back((ticket, class)),
        }
        ticket
    }

    /// 取消尚未获准的票据；已授予的票据无需取消。
    pub(super) fn cancel(&mut self, ticket: GateTicket) {
        self.queue.retain(|(queued, _)| *queued != ticket);
    }

    /// 尚未获准的票据数；只供测试观察被放弃的等待者是否撤票。
    #[cfg(test)]
    pub(super) fn queued(&self) -> usize {
        self.queue.len()
    }

    /// 该票据当前是否仍有资格获得令牌。
    fn is_registered(&self, ticket: GateTicket) -> bool {
        self.queue.iter().any(|(queued, _)| *queued == ticket)
    }

    /// 只有被选中的票据在令牌可用时消耗令牌并推进时间槽。
    pub(super) fn poll(&mut self, ticket: GateTicket, now: Instant) -> GatePoll {
        if !self.is_registered(ticket) {
            return GatePoll::Expired;
        }
        if self.winner() != Some(ticket) {
            // 尚不是本轮胜者：等到下一个可能的时间槽再观察，不做忙轮询。
            return GatePoll::WaitUntil(now.checked_add(self.interval).unwrap_or(now));
        }
        if now < self.next_free {
            return GatePoll::WaitUntil(self.next_free);
        }
        let Some((_, class)) = self.queue.iter().find(|(queued, _)| *queued == ticket).copied()
        else {
            return GatePoll::Expired;
        };
        self.queue.retain(|(queued, _)| *queued != ticket);
        self.foreground_streak = match class {
            GateClass::Foreground => self.foreground_streak.saturating_add(1),
            GateClass::Background => 0,
        };
        // 容量 1：每个令牌只推进一个完整间隔，不积累突发额度。
        self.next_free = now.checked_add(self.interval).unwrap_or(now);
        GatePoll::Granted
    }

    /// 当前应获令牌的票据：保底到期时优先最早的后台请求，否则按队列位置。
    fn winner(&self) -> Option<GateTicket> {
        if self.foreground_streak >= self.floor_after
            && let Some((ticket, _)) =
                self.queue.iter().find(|(_, class)| *class == GateClass::Background)
        {
            return Some(*ticket);
        }
        self.queue.front().map(|(ticket, _)| *ticket)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::time::{Duration, Instant};

    use super::{GateClass, GatePoll, LOW_PRIORITY_FLOOR_AFTER, PriorityGateState};

    #[test]
    fn gate_has_no_burst_and_never_faster_than_interval() {
        let now = Instant::now();
        let interval = Duration::from_millis(500);
        let mut gate = PriorityGateState::new(now, interval, LOW_PRIORITY_FLOOR_AFTER);
        let first = gate.register(GateClass::Foreground);
        assert_eq!(gate.poll(first, now), GatePoll::Granted);

        let second = gate.register(GateClass::Foreground);
        assert_eq!(gate.poll(second, now), GatePoll::WaitUntil(now + interval));
        assert_eq!(
            gate.poll(second, now + interval - Duration::from_millis(1)),
            GatePoll::WaitUntil(now + interval)
        );
        assert_eq!(gate.poll(second, now + interval), GatePoll::Granted);

        // 第三个令牌仍按上一次获准时刻推进，不会因等待时间累积突发额度。
        let third = gate.register(GateClass::Foreground);
        let late = now + interval + Duration::from_secs(10);
        assert_eq!(gate.poll(third, late), GatePoll::Granted);
        assert_eq!(gate.next_free, late + interval);
    }

    #[test]
    fn gate_serves_background_after_floor() {
        let now = Instant::now();
        let interval = Duration::from_millis(100);
        let mut gate = PriorityGateState::new(now, interval, LOW_PRIORITY_FLOOR_AFTER);
        let background = gate.register(GateClass::Background);
        let mut grants = Vec::new();
        let mut at = now;
        // 连续三次前台授予；后台一直排队。
        for _ in 0..3 {
            let foreground = gate.register(GateClass::Foreground);
            match gate.poll(foreground, at) {
                GatePoll::Granted => grants.push(GateClass::Foreground),
                other => panic!("unexpected {other:?}"),
            }
            at += interval;
        }
        // 保底把第四个令牌留给仍在等待的后台，前台不能再插队。
        let extra_foreground = gate.register(GateClass::Foreground);
        assert_matches!(gate.poll(extra_foreground, at), GatePoll::WaitUntil(_));
        assert_eq!(gate.poll(background, at), GatePoll::Granted);
        grants.push(GateClass::Background);
        assert_eq!(
            grants,
            vec![
                GateClass::Foreground,
                GateClass::Foreground,
                GateClass::Foreground,
                GateClass::Background
            ]
        );
    }

    #[test]
    fn gate_floor_does_not_preempt_when_no_background_is_waiting() {
        let now = Instant::now();
        let interval = Duration::from_millis(10);
        let mut gate = PriorityGateState::new(now, interval, 1);
        let mut at = now;
        for _ in 0..5 {
            let foreground = gate.register(GateClass::Foreground);
            assert_eq!(gate.poll(foreground, at), GatePoll::Granted);
            at += interval;
        }
        let background = gate.register(GateClass::Background);
        let foreground = gate.register(GateClass::Foreground);
        assert_eq!(gate.poll(foreground, at), GatePoll::WaitUntil(at + interval));
        assert_eq!(gate.poll(background, at), GatePoll::Granted);
    }

    #[test]
    fn gate_cancel_removes_ticket_without_consuming_token() {
        let now = Instant::now();
        let mut gate =
            PriorityGateState::new(now, Duration::from_millis(50), LOW_PRIORITY_FLOOR_AFTER);
        let cancelled = gate.register(GateClass::Foreground);
        gate.cancel(cancelled);
        assert_eq!(gate.poll(cancelled, now), GatePoll::Expired);
        let next = gate.register(GateClass::Background);
        assert_eq!(gate.poll(next, now), GatePoll::Granted);
    }
}
