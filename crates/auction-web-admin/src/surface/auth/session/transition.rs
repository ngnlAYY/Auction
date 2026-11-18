//! 会话事件归约与副作用的派发条件。

use std::mem;
use std::rc::Rc;

use super::deadlines::{is_newer_projection, session_check_delay, session_lifetime_millis};
use super::{
    Check, CheckReason, Cookie, Effect, Event, Logout, LogoutOutcome, Phase, ReadOutcome,
    RenewalOutcome, Request, State, Ticket, Timer, Verified,
};
use crate::api::AdminSession;
use crate::surface::auth::activity::{RenewalTiming, activity_delay, estimated_expiration_millis};

const CHECK_RETRY_MILLIS: u32 = 30_000;

/// 用单次事件与当前观测推进会话，浏览器适配负责执行返回的动作。
pub(in crate::surface::auth) fn step(
    mut state: State,
    event: Event,
    cookie: Cookie,
    now: f64,
) -> (State, Vec<Effect>) {
    let mut effects = Actions { cookie, now, effects: Vec::new() };
    match event {
        Event::Start if matches!(state.phase, Phase::Starting) => {
            state.begin_read(CheckReason::Startup, &mut effects);
        }
        Event::LoginCompleted(session) if matches!(state.phase, Phase::LoggedOut) => {
            if !Cookie::from_session(&session).conflicts_with(&effects.cookie) {
                state.activity.timing = Some(RenewalTiming::from_session(&session));
            }
            state.begin_read(CheckReason::Login, &mut effects);
        }
        Event::CheckRequested if !matches!(state.phase, Phase::LoggedOut) => {
            state.begin_read(CheckReason::Explicit, &mut effects);
        }
        Event::Broadcast => {
            state.hide_session(&mut effects);
            state.begin_read(CheckReason::Broadcast, &mut effects);
        }
        Event::Visible
            if state.check.is_none()
                && !matches!(state.phase, Phase::LoggedOut | Phase::LoggingOut(..)) =>
        {
            state.begin_read(CheckReason::Routine, &mut effects);
        }
        Event::Input { on_logout: false } if matches!(state.phase, Phase::Authenticated(_)) => {
            state.activity.latest_input = Some(now);
            state.schedule_renewal(&mut effects);
        }
        Event::LogoutClicked => state.start_logout(&mut effects),
        Event::TimerElapsed(timer, ticket) => state.timer_elapsed(timer, ticket, &mut effects),
        Event::ReadFinished(ticket, sent, outcome) => {
            state.finish_read(ticket, &sent, outcome, &mut effects);
        }
        Event::LogoutFinished(ticket, sent, outcome) => {
            state.finish_logout(ticket, &sent, outcome, &mut effects);
        }
        Event::RenewalFinished(ticket, sent, outcome) => {
            state.finish_renewal(ticket, &sent, outcome, &mut effects);
        }
        _ => {}
    }
    (state, effects.effects)
}

struct Actions {
    cookie: Cookie,
    now: f64,
    effects: Vec<Effect>,
}

impl State {
    fn ticket(&mut self) -> Ticket {
        self.next_ticket += 1;
        self.next_ticket
    }

    fn request(&mut self, actions: &Actions) -> Request {
        Request { ticket: self.ticket(), cookie: actions.cookie.clone() }
    }

    fn timer(&mut self, timer: Timer) -> &mut Option<Ticket> {
        match timer {
            Timer::Check => &mut self.check_timer,
            Timer::Renewal => &mut self.renewal_timer,
        }
    }

    fn cancel_timer(&mut self, timer: Timer, actions: &mut Actions) {
        if self.timer(timer).take().is_some() {
            actions.effects.push(Effect::CancelTimer(timer));
        }
    }

    fn arm_timer(&mut self, timer: Timer, delay: u32, actions: &mut Actions) {
        self.cancel_timer(timer, actions);
        let ticket = self.ticket();
        *self.timer(timer) = Some(ticket);
        actions.effects.push(Effect::ArmTimer(timer, ticket, delay));
    }

    fn cancel_read(&mut self, actions: &mut Actions) {
        if self.check.take().is_some() {
            actions.effects.push(Effect::CancelRead);
        }
    }

    fn hide_session(&mut self, actions: &mut Actions) {
        // 旧宿主的草稿、未决命令和弹窗随宿主失效，旧回执不能重挂同一棵树。
        if self.phase.take_verified().is_some() {
            self.view_epoch += 1;
        }
        self.activity = Default::default();
        self.logout_error = None;
        self.cancel_timer(Timer::Renewal, actions);
    }

    fn begin_read(&mut self, reason: CheckReason, actions: &mut Actions) {
        if let Phase::LoggingOut(_, logout) = &mut self.phase {
            // DELETE 等待期间不可用 GET 的快照恢复授权，也不能撤销退出回执的所有权。
            logout.review_after = true;
            return;
        }
        if reason != CheckReason::AfterRenewal
            && self.check.as_ref().is_some_and(|check| {
                check.reason == reason && check.request.cookie == actions.cookie
            })
        {
            return;
        }
        let keeps_permission = reason == CheckReason::Routine
            && matches!(&self.phase, Phase::Authenticated(verified)
                if !verified.cookie.conflicts_with(&actions.cookie));
        self.cancel_read(actions);
        self.cancel_timer(Timer::Check, actions);
        if !keeps_permission {
            self.cancel_timer(Timer::Renewal, actions);
            self.phase = Phase::Checking(self.phase.take_verified());
        }
        let request = self.request(actions);
        actions.effects.push(Effect::Read(request.ticket));
        self.check = Some(Check { reason, request });
    }

    fn reread_current_cookie(&mut self, actions: &mut Actions) {
        if !self
            .phase
            .verified()
            .is_some_and(|verified| verified.cookie.agrees_with(&actions.cookie))
        {
            self.hide_session(actions);
        }
        self.begin_read(CheckReason::CookieChanged, actions);
    }

    fn finish_read(
        &mut self,
        ticket: Ticket,
        sent: &Cookie,
        outcome: ReadOutcome,
        actions: &mut Actions,
    ) {
        let Some(check) = self.check.take_if(|check| check.request.ticket == ticket) else {
            return;
        };
        if check.request.is_stale(sent, &actions.cookie, false) {
            self.reread_current_cookie(actions);
            return;
        }
        match outcome {
            ReadOutcome::Authenticated(session) => {
                let verified = Verified::new(session);
                if verified.cookie.conflicts_with(&actions.cookie)
                    || verified.cookie.conflicts_with(sent)
                {
                    self.reread_current_cookie(actions);
                } else if self
                    .phase
                    .verified()
                    .is_some_and(|previous| !previous.session.same_identity(&verified.session))
                {
                    self.end_session(false, actions);
                } else {
                    self.confirm(verified, actions);
                }
            }
            ReadOutcome::Unauthenticated => {
                self.end_session(self.should_notify_expiration(sent, actions), actions);
            }
            ReadOutcome::Failed(error) => {
                // 临时故障关闭写门禁，但仍保留已确认宿主的草稿和待对账命令。
                self.phase = Phase::Unavailable(self.phase.take_verified(), error);
                self.cancel_timer(Timer::Renewal, actions);
                self.arm_timer(Timer::Check, CHECK_RETRY_MILLIS, actions);
            }
        }
    }

    fn confirm(&mut self, mut verified: Verified, actions: &mut Actions) {
        if let Some(current) = self.phase.verified()
            && current.session.same_identity(&verified.session)
            && !is_newer_projection(&verified.session, &current.session)
        {
            verified.session = Rc::clone(&current.session);
        }
        self.phase = Phase::Authenticated(verified);
        self.schedule_check(actions);
        self.schedule_renewal(actions);
    }

    fn end_session(&mut self, notify: bool, actions: &mut Actions) {
        self.hide_session(actions);
        self.cancel_read(actions);
        self.cancel_timer(Timer::Check, actions);
        self.phase = Phase::LoggedOut;
        if notify {
            actions.effects.push(Effect::BroadcastEnded);
        }
    }

    fn should_notify_expiration(&self, sent: &Cookie, actions: &Actions) -> bool {
        // 到达绝对截止时浏览器也会清空 Cookie。持续缺失与读取失败有不同的代际证据。
        self.phase.verified().is_some()
            && !matches!(sent, Cookie::Unavailable)
            && *sent == actions.cookie
    }

    fn start_logout(&mut self, actions: &mut Actions) {
        if matches!(self.phase, Phase::LoggingOut(..)) {
            return;
        }
        let was_writable = matches!(self.phase, Phase::Authenticated(_));
        let Some(verified) = self.phase.take_verified() else { return };
        if verified.cookie.conflicts_with(&actions.cookie) {
            self.phase = Phase::Checking(Some(verified));
            self.begin_read(CheckReason::Explicit, actions);
            return;
        }
        self.cancel_read(actions);
        self.cancel_timer(Timer::Check, actions);
        self.cancel_timer(Timer::Renewal, actions);
        self.activity.latest_input = None;
        self.logout_error = None;
        let request = self.request(actions);
        actions.effects.push(Effect::Logout(request.ticket, Rc::clone(&verified.session)));
        self.phase = Phase::LoggingOut(
            Some(verified),
            Logout { request, review_after: false, was_writable },
        );
    }

    fn finish_logout(
        &mut self,
        ticket: Ticket,
        sent: &Cookie,
        outcome: LogoutOutcome,
        actions: &mut Actions,
    ) {
        let Phase::LoggingOut(_, logout) = &self.phase else { return };
        if logout.request.ticket != ticket {
            return;
        }
        let success = matches!(outcome, LogoutOutcome::Succeeded);
        let cleared_cookie = success && matches!(actions.cookie, Cookie::Missing);
        let stale = logout.request.is_stale(sent, &actions.cookie, cleared_cookie);
        let review_after = logout.review_after;
        let was_writable = logout.was_writable;
        self.phase = Phase::Checking(self.phase.take_verified());
        if stale {
            self.reread_current_cookie(actions);
            return;
        }
        match outcome {
            LogoutOutcome::Succeeded => {
                self.end_session(true, actions);
                if review_after {
                    self.begin_read(CheckReason::AfterLogout, actions);
                }
            }
            LogoutOutcome::Rejected(error)
                if was_writable && !review_after && sent.agrees_with(&actions.cookie) =>
            {
                self.logout_error = Some(error);
                if let Some(verified) = self.phase.take_verified() {
                    self.confirm(verified, actions);
                }
            }
            LogoutOutcome::Rejected(error)
            | LogoutOutcome::Unknown(error)
            | LogoutOutcome::RateLimited(error) => {
                self.logout_error = Some(error);
                self.begin_read(CheckReason::AfterLogout, actions);
            }
        }
    }

    fn schedule_check(&mut self, actions: &mut Actions) {
        if let Phase::Authenticated(verified) = &self.phase
            && self.check.is_none()
        {
            let session = &verified.session;
            let delay = session_check_delay(
                session_lifetime_millis(session),
                session.received_at_millis,
                actions.now,
            );
            self.arm_timer(Timer::Check, delay, actions);
        }
    }

    fn renewal_delay(&self, now: f64) -> Option<u32> {
        let Phase::Authenticated(verified) = &self.phase else { return None };
        activity_delay(
            self.activity.timing,
            self.activity.latest_input,
            now,
            estimated_expiration_millis(&verified.session),
            self.activity.retry_not_before,
        )
    }

    fn schedule_renewal(&mut self, actions: &mut Actions) {
        if !matches!(self.phase, Phase::Authenticated(_))
            || self.renewal.is_some()
            || self.renewal_timer.is_some()
        {
            return;
        }
        if let Some(delay) = self.renewal_delay(actions.now) {
            self.arm_timer(Timer::Renewal, delay, actions);
        } else {
            self.activity.latest_input = None;
        }
    }

    fn timer_elapsed(&mut self, timer: Timer, ticket: Ticket, actions: &mut Actions) {
        if self.timer(timer).take_if(|armed| *armed == ticket).is_none() {
            return;
        }
        match timer {
            Timer::Check => self.begin_read(CheckReason::Routine, actions),
            Timer::Renewal => self.start_renewal(actions),
        }
    }

    fn start_renewal(&mut self, actions: &mut Actions) {
        let Phase::Authenticated(verified) = &self.phase else { return };
        if verified.cookie.conflicts_with(&actions.cookie) {
            self.begin_read(CheckReason::Explicit, actions);
            return;
        }
        if self.renewal_delay(actions.now) != Some(0) {
            self.schedule_renewal(actions);
            return;
        }
        let session = Rc::clone(&verified.session);
        // 消耗已派发的活动批次。等待回执期间记录的输入属于下一次派发。
        self.activity.latest_input = None;
        let request = self.request(actions);
        actions.effects.push(Effect::Renew(request.ticket, session));
        self.renewal = Some(request);
    }

    fn finish_renewal(
        &mut self,
        ticket: Ticket,
        sent: &Cookie,
        outcome: RenewalOutcome,
        actions: &mut Actions,
    ) {
        let Some(request) = self.renewal.take_if(|request| request.ticket == ticket) else {
            return;
        };
        if request.is_stale(sent, &actions.cookie, false) {
            self.reread_current_cookie(actions);
            return;
        }
        match outcome {
            RenewalOutcome::Succeeded(session) => {
                self.apply_renewal(session, sent, actions);
            }
            RenewalOutcome::Unauthenticated => {
                if matches!(self.phase, Phase::LoggingOut(..)) {
                    self.hide_session(actions);
                    self.begin_read(CheckReason::AfterRenewal, actions);
                } else {
                    self.end_session(self.should_notify_expiration(sent, actions), actions);
                }
            }
            RenewalOutcome::RateLimited(retry_after) => {
                let delay = retry_after.map_or_else(
                    || self.activity.timing.map_or(5_000.0, |timing| timing.interval_millis),
                    |millis| millis as f64,
                );
                self.activity.retry_not_before =
                    self.activity.retry_not_before.max(actions.now + delay);
                // 只可按退避下界安排已记录的新输入，已被拒绝的请求没有可重发批次。
                self.schedule_renewal(actions);
            }
            RenewalOutcome::CsrfRejected | RenewalOutcome::Unknown => {
                self.activity.latest_input = None;
                // 即使写门禁已关闭，在途 POST 也可能已经提交。只启动一次有预算的 GET。
                self.begin_read(CheckReason::AfterRenewal, actions);
            }
            RenewalOutcome::Rejected => self.activity.latest_input = None,
        }
    }

    fn apply_renewal(&mut self, session: Rc<AdminSession>, sent: &Cookie, actions: &mut Actions) {
        let projection_cookie = Cookie::from_session(&session);
        if projection_cookie.conflicts_with(&actions.cookie)
            || projection_cookie.conflicts_with(sent)
        {
            self.reread_current_cookie(actions);
            return;
        }
        if matches!(self.phase, Phase::LoggingOut(..)) {
            return;
        }
        if let Some(verified) = self.phase.verified_mut()
            && verified.session.same_identity(&session)
        {
            self.activity.timing = Some(RenewalTiming::from_session(&session));
            self.activity.retry_not_before = 0.0;
            if is_newer_projection(&session, &verified.session) {
                verified.session = session;
            }
            self.schedule_check(actions);
            self.schedule_renewal(actions);
        }
    }
}

impl Phase {
    fn take_verified(&mut self) -> Option<Verified> {
        match self {
            Self::Authenticated(_) => match mem::take(self) {
                Self::Authenticated(verified) => Some(verified),
                _ => None,
            },
            Self::Checking(verified)
            | Self::Unavailable(verified, _)
            | Self::LoggingOut(verified, _) => verified.take(),
            Self::Starting | Self::LoggedOut => None,
        }
    }

    fn verified_mut(&mut self) -> Option<&mut Verified> {
        match self {
            Self::Authenticated(verified) => Some(verified),
            Self::Checking(verified) | Self::Unavailable(verified, _) => verified.as_mut(),
            Self::Starting | Self::LoggingOut(..) | Self::LoggedOut => None,
        }
    }
}
