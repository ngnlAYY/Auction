//! 会话事件归约与副作用的派发条件。

use std::mem;
use std::rc::Rc;

use super::deadlines::{is_newer_projection, session_check_delay, session_lifetime_millis};
use super::{
    Check, CheckReason, Cookie, Effect, Event, Phase, Renewal, RenewalOutcome, Request, State,
    Ticket, Timer, Verified,
};
use crate::api::AdminSession;
use crate::surface::auth::activity::{RenewalTiming, activity_delay, estimated_expiration_millis};

/// 用单次事件与当前观测推进会话，浏览器适配负责执行返回的动作。
/// 当前 Cookie 先经过身份观测；回执携带的实发代际只用于核对自己的归属。
pub(in crate::surface::auth) fn step(
    mut state: State,
    event: Event,
    cookie: Cookie,
    now: f64,
) -> (State, Vec<Effect>) {
    if matches!(&event, Event::WriteReviewIdentity { epoch, .. } if *epoch != state.view_epoch) {
        // 旧宿主的回调连当前 Cookie 观测也不能送入新宿主。
        return (state, Vec::new());
    }
    if let Event::ExternalUnauthenticated { epoch, session } = &event
        && (*epoch != state.view_epoch
            || !state
                .phase
                .verified()
                .is_some_and(|current| current.session.same_identity(session)))
    {
        return (state, Vec::new());
    }
    let mut effects = Actions { cookie, now, effects: Vec::new() };
    let keys_were_hidden = state.keys_hidden();
    let revision = state.key_visibility_revision;
    let completing_read =
        matches!(&event, Event::ReadFinished(ticket, ..) if state.has_read(*ticket));
    let cookie_changed = state.observe_identity(&mut effects);
    match event {
        Event::Start if matches!(state.phase, Phase::Starting) => {
            state.begin_read(CheckReason::Startup, &mut effects);
        }
        Event::LoginFinished(ticket, outcome) if state.login == Some(ticket) => {
            state.login = None;
            // POST 成功或结果未知都只复核一次当前 Cookie，回执不能授予写权限。
            match outcome {
                Ok(session) => {
                    let origin = state.notice_origin().cloned();
                    state.observe_session_body(origin.as_deref(), &session, &mut effects);
                    if Cookie::from_session(&session).agrees_with(&effects.cookie) {
                        state.activity.timing = Some(RenewalTiming::from_session(&session));
                    }
                }
                Err(code) => state.login_error = Some(code),
            }
            state.begin_read(CheckReason::Login, &mut effects);
        }
        Event::LoginInputChanged
            if matches!(state.phase, Phase::LoggedOut) && state.login.is_none() =>
        {
            state.login_error = None;
        }
        Event::CheckRequested if !matches!(state.phase, Phase::LoggedOut) => {
            state.begin_read(CheckReason::Explicit, &mut effects);
        }
        Event::WriteReviewIdentity { session, .. } => {
            let origin = state.phase.verified().map(|verified| Rc::clone(&verified.session));
            if state.observe_session_body(origin.as_deref(), &session, &mut effects) {
                state.begin_read(CheckReason::Explicit, &mut effects);
            }
        }
        Event::ExternalUnauthenticated { session, .. } => {
            let stay = state.check.as_ref().is_some_and(|pending| {
                pending.reason == CheckReason::StayAfterLogout
                    && pending.request.cookie == effects.cookie
            });
            // 401 之前发出的 GET 与续期不能恢复授权。新票保留同代「留在后台」的人工决定。
            state.mark_identity_unconfirmed(false, &mut effects);
            state.defer_expiration(session);
            state.cancel_read(&mut effects);
            let reason = if stay { CheckReason::StayAfterLogout } else { CheckReason::Explicit };
            state.begin_read(reason, &mut effects);
        }
        Event::Broadcast => {
            state.remember_peer_end();
            if state.check.as_ref().is_some_and(|check| check.reason == CheckReason::Login) {
                state.begin_read(CheckReason::Login, &mut effects);
            } else if matches!(state.phase, Phase::LoggedOut) && state.login.is_none() {
                state.begin_read(CheckReason::Discover, &mut effects);
            } else {
                state.hide_session(&mut effects);
                state.begin_read(CheckReason::Broadcast, &mut effects);
            }
        }
        Event::Visible => {
            let reason = if matches!(state.phase, Phase::LoggedOut) {
                CheckReason::Discover
            } else {
                CheckReason::Routine
            };
            state.begin_read(reason, &mut effects);
        }
        Event::Input { on_logout: false } if matches!(state.phase, Phase::Authenticated(_)) => {
            state.activity.latest_input = Some(now);
            state.schedule_renewal(&mut effects);
        }
        Event::LogoutClicked if !state.logout_unconfirmed() => {
            state.start_logout(false, &mut effects);
        }
        Event::ContinueLogout if state.can_decide_logout() => {
            state.start_logout(true, &mut effects);
        }
        Event::StayAfterLogout if state.can_decide_logout() => {
            state.begin_read(CheckReason::StayAfterLogout, &mut effects);
        }
        Event::TimerElapsed(timer, ticket) => state.timer_elapsed(timer, ticket, &mut effects),
        Event::ReadSending(ticket) => state.prepare_read(ticket, &mut effects),
        Event::ReadFinished(ticket, sent, outcome) => {
            state.finish_read(ticket, &sent, outcome, &mut effects);
        }
        Event::LogoutSending(ticket) => state.prepare_logout(ticket, &mut effects),
        Event::LogoutFinished(ticket, sent, outcome) => {
            state.finish_logout(ticket, &sent, outcome, true, &mut effects);
        }
        Event::LogoutNotSent(ticket, sent, outcome) => {
            state.finish_logout(ticket, &sent, outcome, false, &mut effects);
        }
        Event::RenewalSending(ticket) => state.prepare_renewal(ticket, &mut effects),
        Event::RenewalFinished(ticket, sent, outcome) => {
            state.finish_renewal(ticket, &sent, outcome, &mut effects);
        }
        _ => {}
    }
    // 当前读取自行处理权威结论、重读或退避。其他事件即使已过时，观测到换代也要复核。
    if cookie_changed && state.identity_unconfirmed && !completing_read {
        state.begin_read(CheckReason::Routine, &mut effects);
    }
    if !keys_were_hidden && state.keys_hidden() && state.key_visibility_revision == revision {
        // 即使界面合并了两次渲染，旧的打开动作也不能在复核成功后重新显示明文。
        state.key_visibility_revision += 1;
    }
    (state, effects.effects)
}

/// 单次事件的观测值与顺序执行的副作用队列。
pub(super) struct Actions {
    pub(super) cookie: Cookie,
    now: f64,
    pub(super) effects: Vec<Effect>,
}

impl State {
    /// 在外壳派发一次登录 POST 前登记归属，状态机不持有登录凭据。
    pub(in crate::surface::auth) fn start_login(&mut self) -> Option<Ticket> {
        if !matches!(self.phase, Phase::LoggedOut) || self.login.is_some() {
            return None;
        }
        let ticket = self.ticket();
        self.login = Some(ticket);
        self.login_error = None;
        Some(ticket)
    }

    fn ticket(&mut self) -> Ticket {
        self.next_ticket += 1;
        self.next_ticket
    }

    /// 以本轮观测为请求登记代际。
    pub(super) fn request(&mut self, actions: &Actions) -> Request {
        Request { ticket: self.ticket(), cookie: actions.cookie.clone() }
    }

    fn timer(&mut self, timer: Timer) -> &mut Option<Ticket> {
        match timer {
            Timer::Check => &mut self.check_timer,
            Timer::Renewal => &mut self.renewal_timer,
        }
    }

    /// 撤销定时器票与浏览器任务。
    pub(super) fn cancel_timer(&mut self, timer: Timer, actions: &mut Actions) {
        if self.timer(timer).take().is_some() {
            actions.effects.push(Effect::CancelTimer(timer));
        }
    }

    /// 用新票替换同用途的定时器。
    pub(super) fn arm_timer(&mut self, timer: Timer, delay: u32, actions: &mut Actions) {
        self.cancel_timer(timer, actions);
        let ticket = self.ticket();
        *self.timer(timer) = Some(ticket);
        actions.effects.push(Effect::ArmTimer(timer, ticket, delay));
    }

    /// 撤销读取票与浏览器任务。
    pub(super) fn cancel_read(&mut self, actions: &mut Actions) {
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
        self.identity_unconfirmed = false;
        self.saw_other_generation = false;
        self.cancel_timer(Timer::Renewal, actions);
    }

    fn observe_identity(&mut self, actions: &mut Actions) -> bool {
        let cookie = &actions.cookie;
        if let Cookie::Present(token) = cookie {
            self.observe_logout_identity(|target| target.cookie.conflicts_with(cookie));
            self.observe_end_notice_identity(|session| {
                token.as_ref() != session.csrf_token.as_str()
            });
        }
        // 浏览器已切换代际时，旧续期的投影既不能暂存，也不能被新宿主采用。
        self.buffered_renewal.take_if(|pending| pending.cookie.conflicts_with(&actions.cookie));
        if let Some(renewal) = &mut self.renewal {
            renewal.source.take_if(|source| source.cookie.conflicts_with(&actions.cookie));
        }
        if !self
            .phase
            .verified()
            .is_some_and(|verified| verified.cookie.conflicts_with(&actions.cookie))
        {
            return false;
        }
        self.mark_identity_unconfirmed(matches!(actions.cookie, Cookie::Present(_)), actions);
        true
    }

    /// 只接受已核票、归属原宿主的有效正文。异代证据撤回展示和推断资格，不能授权。
    pub(super) fn observe_session_body(
        &mut self,
        origin: Option<&AdminSession>,
        session: &AdminSession,
        actions: &mut Actions,
    ) -> bool {
        let Some(origin) = origin.filter(|origin| !origin.same_identity(session)) else {
            return false;
        };
        self.observe_logout_identity(|target| target.session.same_identity(origin));
        self.observe_end_notice_identity(|target| target.same_identity(origin));
        if self.phase.verified().is_some_and(|verified| verified.session.same_identity(origin)) {
            self.mark_identity_unconfirmed(true, actions);
        }
        true
    }

    fn mark_identity_unconfirmed(&mut self, saw_other: bool, actions: &mut Actions) {
        if !self.identity_unconfirmed {
            self.identity_unconfirmed = true;
            // 后续即便合并了渲染并重新确认 A，旧的打开动作也不能恢复明文。
            self.key_visibility_revision += 1;
        }
        self.saw_other_generation |= saw_other;
        if matches!(self.phase, Phase::Authenticated(_)) {
            self.phase = Phase::Checking(self.phase.take_verified());
        }
        self.retire_renewal();
        self.activity.latest_input = None;
        self.cancel_timer(Timer::Renewal, actions);
    }

    /// 按本轮已处理的 Cookie 代际合并在途读取，保留本地登录的优先级。
    pub(super) fn begin_read(&mut self, mut reason: CheckReason, actions: &mut Actions) {
        if self.login.is_some() {
            // 广播立即卸载旧表单。等待 POST 结清，再合并复核，避免 GET 抢在新 Cookie 之前。
            if reason == CheckReason::Broadcast {
                self.phase = Phase::Checking(None);
            }
            return;
        }
        if let Phase::LoggingOut(_, logout) = &mut self.phase {
            // DELETE 等待期间不可用 GET 的快照恢复授权，也不能撤销退出回执的所有权。
            logout.review_after |= reason != CheckReason::Routine || self.identity_unconfirmed;
            return;
        }
        if let Some(check) = &self.check {
            if check.reason == CheckReason::Login {
                if matches!(actions.cookie, Cookie::Unavailable)
                    || check.request.cookie == actions.cookie
                {
                    return;
                }
                reason = CheckReason::Login;
            }
            if (reason == CheckReason::Routine && matches!(actions.cookie, Cookie::Unavailable))
                || (check.request.cookie == actions.cookie
                    && (matches!(reason, CheckReason::Routine | CheckReason::Discover)
                        || (reason == CheckReason::Explicit
                            && check.reason == CheckReason::StayAfterLogout)
                        || (check.reason == reason && reason != CheckReason::AfterRenewal)))
            {
                return;
            }
        }
        let keeps_permission = reason == CheckReason::Routine
            && matches!(&self.phase, Phase::Authenticated(verified)
                if !verified.cookie.conflicts_with(&actions.cookie));
        self.cancel_read(actions);
        self.cancel_timer(Timer::Check, actions);
        if !keeps_permission && !self.logout_unconfirmed() && reason != CheckReason::Discover {
            self.cancel_timer(Timer::Renewal, actions);
            self.phase = if reason == CheckReason::Login
                && matches!(self.phase, Phase::LoggedOut | Phase::CheckingLogin)
            {
                Phase::CheckingLogin
            } else {
                Phase::Checking(self.phase.take_verified())
            };
        }
        let request = self.request(actions);
        actions.effects.push(Effect::Read(request.ticket));
        let logout_ticket = self.logout_intent.as_ref().map(|intent| intent.last_sent);
        let origin = self.notice_origin().cloned();
        self.check = Some(Check { reason, request, origin, logout_ticket });
    }

    /// 采用同身份的已确认投影并恢复日常复核与续期。
    pub(super) fn confirm(&mut self, mut verified: Verified, actions: &mut Actions) {
        if let Some(current) = self.phase.verified()
            && current.session.same_identity(&verified.session)
            && !is_newer_projection(&verified.session, &current.session)
        {
            verified.session = Rc::clone(&current.session);
        }
        if let Some(renewal) = &mut self.renewal {
            renewal.source.take_if(|source| !source.session.same_identity(&verified.session));
        }
        self.login_error = None;
        self.logout_intent = None;
        self.clear_pending_end_notices();
        self.retain_settled_end_notice(&verified.session);
        self.identity_unconfirmed = false;
        self.saw_other_generation = false;
        self.phase = Phase::Authenticated(verified);
        self.schedule_check(actions);
        self.schedule_renewal(actions);
    }

    /// 撤下认证宿主并结清退出流程。通知归属由独立账本判断。
    pub(super) fn end_session(&mut self, actions: &mut Actions) {
        self.hide_session(actions);
        self.logout_intent = None;
        self.retire_renewal();
        self.cancel_read(actions);
        self.cancel_timer(Timer::Check, actions);
        self.phase = Phase::LoggedOut;
    }

    /// 撤销续期的投影资格，保留原票与 epoch 供失效证据核对。
    pub(super) fn retire_renewal(&mut self) {
        self.buffered_renewal = None;
        if let Some(renewal) = &mut self.renewal {
            renewal.source = None;
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
        if self.renewal_delay(actions.now) != Some(0) {
            self.schedule_renewal(actions);
            return;
        }
        let source = verified.clone();
        // 消耗已派发的活动批次。等待回执期间记录的输入属于下一次派发。
        self.activity.latest_input = None;
        let request = self.request(actions);
        actions.effects.push(Effect::Renew(request.ticket, Rc::clone(&source.session)));
        self.renewal = Some(Renewal {
            request,
            origin: Rc::clone(&source.session),
            source: Some(source),
            epoch: self.view_epoch,
        });
    }

    fn prepare_renewal(&mut self, ticket: Ticket, actions: &mut Actions) {
        let Some(renewal) = &self.renewal else { return };
        if renewal.request.ticket != ticket {
            return;
        }
        let same_host = matches!(&self.phase, Phase::Authenticated(current)
            if renewal.source.as_ref().is_some_and(|source|
                source.session.same_identity(&current.session)));
        if !same_host || renewal.request.cookie.conflicts_with(&actions.cookie) {
            self.renewal = None;
            self.schedule_renewal(actions);
        }
    }

    fn finish_renewal(
        &mut self,
        ticket: Ticket,
        sent: &Cookie,
        outcome: RenewalOutcome,
        actions: &mut Actions,
    ) {
        let Some(renewal) = self.renewal.take_if(|renewal| renewal.request.ticket == ticket) else {
            return;
        };
        if self.phase.verified().is_some() && renewal.epoch != self.view_epoch {
            self.schedule_renewal(actions);
            return;
        }
        // Cookie 冲突可以撤销投影资格。当前票和原宿主仍匹配时，正文依然是身份证据。
        if let RenewalOutcome::Succeeded(session) = &outcome {
            let origin =
                self.notice_origin().filter(|owner| owner.same_identity(&renewal.origin)).cloned();
            if self.observe_session_body(origin.as_deref(), session, actions) {
                self.activity.latest_input = None;
                self.begin_read(CheckReason::AfterRenewal, actions);
                return;
            }
        }
        let request_matches = !renewal.request.is_stale(sent, &actions.cookie, false);
        let projection_allowed = request_matches
            && renewal.source.as_ref().is_some_and(|source| !source.cookie.conflicts_with(sent));
        // 退出限流等迁移会撤销续期投影。原票的可信 401 仍能结束同一 epoch 的会话。
        let owned_expiration = matches!(outcome, RenewalOutcome::Unauthenticated)
            && request_matches
            && *sent == actions.cookie
            && matches!(sent, Cookie::Present(token) if token.as_ref() == renewal.origin.csrf_token.as_str())
            && (projection_allowed
                || (renewal.epoch == self.view_epoch
                    && self
                        .phase
                        .verified()
                        .is_some_and(|current| current.session.same_identity(&renewal.origin))));
        if !projection_allowed && !owned_expiration {
            // 旧回执不接管已有的登录、复核或退避；新输入仍由当前已确认会话排期。
            self.schedule_renewal(actions);
            return;
        }
        match outcome {
            RenewalOutcome::Succeeded(session) if renewal.origin.same_identity(&session) => {
                self.apply_renewal(session, sent, actions);
            }
            RenewalOutcome::Succeeded(_) => {}
            RenewalOutcome::Unauthenticated => {
                if matches!(self.phase, Phase::LoggingOut(..)) {
                    if owned_expiration {
                        self.defer_expiration(renewal.origin);
                    }
                    self.hide_session(actions);
                    self.begin_read(CheckReason::AfterRenewal, actions);
                } else {
                    if owned_expiration {
                        self.settle_end_notice(&renewal.origin, actions);
                    }
                    // 使 401 之前的 GET 票失效，只有新读取可以重新确认写权限。
                    self.end_session(actions);
                    if !projection_allowed {
                        self.begin_read(CheckReason::Discover, actions);
                    }
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
            self.begin_read(CheckReason::CookieChanged, actions);
            return;
        }
        if matches!(self.phase, Phase::Checking(None) | Phase::Unavailable(None, _)) {
            // 只暂存来源身份及实际 Cookie 都匹配的回执。敏感宿主和写许可仍由 GET 确认。
            if projection_cookie.agrees_with(sent) && projection_cookie.agrees_with(&actions.cookie)
            {
                self.buffered_renewal = Some(Verified { session, cookie: projection_cookie });
            }
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
    /// 转移当前认证宿主的所有权。
    pub(super) fn take_verified(&mut self) -> Option<Verified> {
        match self {
            Self::Authenticated(_) => match mem::take(self) {
                Self::Authenticated(verified) => Some(verified),
                _ => None,
            },
            Self::Checking(verified)
            | Self::Unavailable(verified, _)
            | Self::LoggingOut(verified, _)
            | Self::LogoutUnconfirmed(verified) => verified.take(),
            Self::Starting | Self::CheckingLogin | Self::LoggedOut => None,
        }
    }

    fn verified_mut(&mut self) -> Option<&mut Verified> {
        match self {
            Self::Authenticated(verified) => Some(verified),
            Self::Checking(verified) | Self::Unavailable(verified, _) => verified.as_mut(),
            Self::Starting
            | Self::CheckingLogin
            | Self::LoggingOut(..)
            | Self::LogoutUnconfirmed(_)
            | Self::LoggedOut => None,
        }
    }
}
