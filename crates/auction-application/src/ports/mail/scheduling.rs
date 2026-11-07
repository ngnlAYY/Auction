// 外部账号工作的调度策略：出队优先级、请求门（容量 1 的令牌桶、低优先级保底）、
// 独占操作锁排队与长列表扫描分段。全部为纯计算，只决定先后，不决定业务结果、证明、
// 租约或重试；持久 outbox 与 sending 围栏仍是唯一权威。
//
// 优先级沿用决策 14 / 61：运维告警固定最高，其次是账号类回信（登录 key、注册与换绑结果），
// 出价被超提醒按 lot 有效截止先后排在其他类别之前；只读轮询与维护属于最低档，保底规则
// 只保证它们不会在持续写入下被无限推迟，不提高总速率。

use std::{
    collections::VecDeque,
    ops::Range,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};

/// 出队优先级；仅用于排序，不改变任何邮件、投递或命令的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountWorkPriority {
    /// 运维告警，固定最高。
    OperationsAlert,
    /// 账号注册、登录 Key 与换绑结果。
    AccountReply,
    /// 出价被超提醒；`deadline` 为 lot 有效截止，越早越先发。
    Outbid { deadline: Option<DateTime<Utc>> },
    /// 结算、装备、收件与其他业务邮件。
    Other,
    /// 收件轮询、Sent/Read Mail 扫描与维护等只读工作。
    BackgroundRead,
}

impl AccountWorkPriority {
    /// 数值越小越先出队。
    pub const fn rank(self) -> u8 {
        match self {
            Self::OperationsAlert => 0,
            Self::AccountReply => 1,
            Self::Outbid { .. } => 2,
            Self::Other => 3,
            Self::BackgroundRead => 4,
        }
    }

    /// lot 有效截止；只有出价被超提醒携带。
    pub const fn deadline(self) -> Option<DateTime<Utc>> {
        match self {
            Self::Outbid { deadline } => deadline,
            _ => None,
        }
    }

    /// 从持久投递的 origin 与 payload 模板投影出队优先级。
    /// 分类口径与数据库 `public.delivery_send_category` 保持一致；本函数不推断未绑定 lot 的截止。
    pub fn from_delivery(origin_kind: &str, template: Option<&str>) -> Self {
        match origin_kind {
            "operations_alert" => Self::OperationsAlert,
            "lot_watch" => Self::Outbid { deadline: None },
            "mail_intake" => match template {
                Some(
                    "register_key"
                    | "register_result"
                    | "rekey_key"
                    | "rekey_result"
                    | "rebind_result"
                    | "user_assets_rejected",
                ) => Self::AccountReply,
                _ => Self::Other,
            },
            _ => Self::Other,
        }
    }

    /// 补充出价被超提醒的 lot 有效截止；其他类别原样返回。
    pub const fn with_deadline(self, deadline: Option<DateTime<Utc>>) -> Self {
        match self {
            Self::Outbid { .. } => Self::Outbid { deadline },
            other => other,
        }
    }
}

impl Ord for AccountWorkPriority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank()).then_with(|| match (self.deadline(), other.deadline()) {
            // 越早的截止越先；缺少截止的提醒排在同类末尾。
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
    }
}

impl PartialOrd for AccountWorkPriority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 请求门的两档：业务写入优先于只读轮询；保底规则只作用于最低档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateClass {
    /// compose、资产动作与最终发送等业务写入。
    Foreground,
    /// 收件箱/已读列表、Sent 扫描、余额与维护等只读请求。
    Background,
}

/// 连续前台授予达到该值后，下一个令牌必须给排队的后台请求。
pub const LOW_PRIORITY_FLOOR_AFTER: u32 = 3;

/// 已登记的令牌请求；只在本进程内有效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateTicket(u64);

impl GateTicket {
    pub const fn id(self) -> u64 {
        self.0
    }
}

/// 单个等待者对令牌的观察结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatePoll {
    /// 已获得唯一令牌；调用方可以继续发出一次请求。
    Granted,
    /// 尚未轮到；在该时刻之前不应再次忙轮询。
    WaitUntil(Instant),
    /// 票据已被取消（等待者被丢弃）；调用方必须放弃本次请求。
    Expired,
}

/// 容量为 1 的分级令牌桶状态机：无突发、总量不超过配置间隔、后台请求按保底获得份额。
#[derive(Debug)]
pub struct PriorityGateState {
    interval: Duration,
    floor_after: u32,
    next_free: Instant,
    foreground_streak: u32,
    next_ticket: u64,
    queue: VecDeque<(GateTicket, GateClass)>,
}

impl PriorityGateState {
    /// `interval` 必须为正；`floor_after` 为 0 表示不设保底（不推荐）。
    pub fn new(now: Instant, interval: Duration, floor_after: u32) -> Self {
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
    pub fn register(&mut self, class: GateClass) -> GateTicket {
        let ticket = GateTicket(self.next_ticket);
        self.next_ticket = self.next_ticket.wrapping_add(1);
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
    pub fn cancel(&mut self, ticket: GateTicket) {
        self.queue.retain(|(queued, _)| *queued != ticket);
    }

    /// 该票据当前是否仍有资格获得令牌。
    pub fn is_registered(&self, ticket: GateTicket) -> bool {
        self.queue.iter().any(|(queued, _)| *queued == ticket)
    }

    /// 下一次可能有令牌可用的时刻；仅用于唤醒，不是授权。
    pub const fn next_free(&self) -> Instant {
        self.next_free
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// 只有被选中的票据在令牌可用时消耗令牌并推进时间槽。
    pub fn poll(&mut self, ticket: GateTicket, now: Instant) -> GatePoll {
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

/// 独占操作锁（compose/资产动作）的纯排队状态；同一时刻只有一个持有者。
/// 长扫描不得登记到这把锁：只读工作不改变远端成员关系，阻塞在锁上只会拖慢写入。
#[derive(Debug, Default)]
pub struct OperationLockState {
    held: bool,
    next_ticket: u64,
    queue: VecDeque<(u64, GateClass)>,
}

impl OperationLockState {
    /// 前台写入排在后台维护之前；同类按登记顺序。
    pub fn register(&mut self, class: GateClass) -> u64 {
        let ticket = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1);
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

    /// 取消尚未获准的登记；等待者被丢弃时必须调用，避免空票据阻塞后续请求。
    pub fn cancel(&mut self, ticket: u64) {
        self.queue.retain(|(queued, _)| *queued != ticket);
    }

    /// 仅当锁空闲且该票据位于队首时获准。
    pub fn try_acquire(&mut self, ticket: u64) -> bool {
        if self.held || self.queue.front().map(|(queued, _)| *queued) != Some(ticket) {
            return false;
        }
        self.queue.pop_front();
        self.held = true;
        true
    }

    /// 释放唯一持有者；只有持锁 guard 的 Drop 可以调用。
    pub fn release(&mut self) {
        self.held = false;
    }

    pub const fn is_held(&self) -> bool {
        self.held
    }

    pub fn waiting(&self) -> usize {
        self.queue.len()
    }
}

/// 长列表扫描的分段游标；每段之间必须释放成员关系读锁，让详情 GET 有机会推进。
#[derive(Debug, Clone)]
pub struct SegmentPlan {
    total_pages: usize,
    segment_pages: usize,
    next_page: usize,
}

impl SegmentPlan {
    /// `segment_pages` 必须为正；段数由 `total_pages` 决定。
    pub fn new(total_pages: usize, segment_pages: usize) -> Self {
        assert!(segment_pages > 0, "segment_pages must be positive");
        Self { total_pages, segment_pages, next_page: 0 }
    }

    /// 返回下一个不重叠的页区间；耗尽后返回 `None`。
    pub fn next_segment(&mut self) -> Option<Range<usize>> {
        if self.next_page >= self.total_pages {
            return None;
        }
        let start = self.next_page;
        let end = start.saturating_add(self.segment_pages).min(self.total_pages);
        self.next_page = end;
        Some(start..end)
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::ports::mail::scheduling as parent;
    use chrono::TimeZone;

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).single().expect("timestamp")
    }

    #[test]
    fn priority_order_follows_decisions_14_and_61() {
        let later = AccountWorkPriority::Outbid { deadline: Some(at(200)) };
        let earlier = AccountWorkPriority::Outbid { deadline: Some(at(100)) };
        let no_deadline = AccountWorkPriority::Outbid { deadline: None };
        let mut ordered = vec![
            AccountWorkPriority::BackgroundRead,
            AccountWorkPriority::Other,
            later,
            AccountWorkPriority::AccountReply,
            AccountWorkPriority::OperationsAlert,
            no_deadline,
            earlier,
        ];
        ordered.sort();
        assert_eq!(
            ordered,
            vec![
                AccountWorkPriority::OperationsAlert,
                AccountWorkPriority::AccountReply,
                earlier,
                later,
                no_deadline,
                AccountWorkPriority::Other,
                AccountWorkPriority::BackgroundRead,
            ]
        );
    }

    #[test]
    fn delivery_priority_matches_send_category() {
        assert_eq!(
            AccountWorkPriority::from_delivery("operations_alert", Some("alert")),
            AccountWorkPriority::OperationsAlert
        );
        assert_eq!(
            AccountWorkPriority::from_delivery("lot_watch", Some("lot_watch")),
            AccountWorkPriority::Outbid { deadline: None }
        );
        for template in
            ["register_key", "register_result", "rekey_key", "rekey_result", "rebind_result"]
        {
            assert_eq!(
                AccountWorkPriority::from_delivery("mail_intake", Some(template)),
                AccountWorkPriority::AccountReply
            );
        }
        assert_eq!(
            AccountWorkPriority::from_delivery("mail_intake", Some("equipment_delivered")),
            AccountWorkPriority::Other
        );
        assert_eq!(
            AccountWorkPriority::from_delivery("settlement", Some("payment")),
            AccountWorkPriority::Other
        );
        let outbid = AccountWorkPriority::Outbid { deadline: None }.with_deadline(Some(at(5)));
        assert_eq!(outbid.deadline(), Some(at(5)));
        assert_eq!(
            AccountWorkPriority::Other.with_deadline(Some(at(5))),
            AccountWorkPriority::Other
        );
    }

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
        assert_eq!(gate.next_free(), late + interval);
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
        assert!(matches!(gate.poll(extra_foreground, at), GatePoll::WaitUntil(_)));
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

    #[test]
    fn operation_lock_serializes_and_prioritizes_foreground() {
        let mut lock = OperationLockState::default();
        let background = lock.register(GateClass::Background);
        let foreground = lock.register(GateClass::Foreground);
        assert!(!lock.try_acquire(background));
        assert!(lock.try_acquire(foreground));
        assert!(!lock.try_acquire(background));
        lock.release();
        assert!(lock.try_acquire(background));
        assert!(lock.is_held());
        lock.release();
        assert!(!lock.is_held());
    }

    #[test]
    fn operation_lock_cancel_does_not_block_queue() {
        let mut lock = OperationLockState::default();
        let cancelled = lock.register(GateClass::Foreground);
        let next = lock.register(GateClass::Foreground);
        lock.cancel(cancelled);
        assert!(lock.try_acquire(next));
    }

    #[test]
    fn segment_plan_partitions_pages_without_overlap() {
        let mut plan = SegmentPlan::new(10, 4);
        assert_eq!(plan.next_segment(), Some(0..4));
        assert_eq!(plan.next_segment(), Some(4..8));
        assert_eq!(plan.next_segment(), Some(8..10));
        assert_eq!(plan.next_segment(), None);

        let mut empty = SegmentPlan::new(0, 4);
        assert_eq!(empty.next_segment(), None);

        let mut short = SegmentPlan::new(2, 8);
        assert_eq!(short.next_segment(), Some(0..2));
        assert_eq!(short.next_segment(), None);
    }
}
