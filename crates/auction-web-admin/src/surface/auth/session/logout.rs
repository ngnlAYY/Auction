//! 本地退出意图、发送前围栏与结束通知归属。

use std::rc::Rc;

use super::transition::Actions;
use super::{
    Check, CheckReason, Cookie, Effect, Logout, LogoutIntent, LogoutOutcome, Phase, State, Ticket,
    Timer, Verified,
};

impl State {
    /// 退出结果存疑时，同一轮只接受一个人工决定。
    pub(super) fn can_decide_logout(&self) -> bool {
        matches!(self.phase, Phase::LogoutUnconfirmed(_)) && self.check.is_none()
    }

    /// 冻结本次 DELETE 的目标；人工继续时只有原意图的可读代际才能发送。
    pub(super) fn start_logout(&mut self, continuing: bool, actions: &mut Actions) {
        if matches!(self.phase, Phase::LoggingOut(..)) {
            return;
        }
        let Some(verified) = self.phase.verified() else {
            if continuing {
                self.begin_read(CheckReason::AfterLogout, actions);
            }
            return;
        };
        let may_send = if continuing {
            self.logout_intent.as_ref().is_some_and(|intent| {
                intent.target.session.same_identity(&verified.session)
                    && intent.target.cookie.agrees_with(&actions.cookie)
            })
        } else {
            !self.identity_unconfirmed
                && self.logout_intent.is_none()
                && !verified.cookie.conflicts_with(&actions.cookie)
        };
        if !may_send {
            self.begin_read(CheckReason::Explicit, actions);
            return;
        }
        let was_writable = matches!(self.phase, Phase::Authenticated(_));
        let verified = verified.clone();
        self.cancel_read(actions);
        self.cancel_timer(Timer::Check, actions);
        self.cancel_timer(Timer::Renewal, actions);
        self.buffered_renewal = None;
        self.activity.latest_input = None;
        self.logout_error = None;
        let request = self.request(actions);
        actions.effects.push(Effect::Logout(request.ticket, Rc::clone(&verified.session)));
        self.phase = Phase::LoggingOut(
            Some(verified.clone()),
            Logout {
                request,
                target: verified,
                saw_other_generation: false,
                review_after: false,
                was_writable,
                continuing,
            },
        );
    }

    /// 处理实发前的观测后，只有当前票和原身份仍匹配才能派发 DELETE。
    pub(super) fn prepare_logout(&mut self, ticket: Ticket, actions: &mut Actions) {
        let Phase::LoggingOut(verified, logout) = &mut self.phase else { return };
        if logout.request.ticket != ticket {
            return;
        }
        let sent = &actions.cookie;
        let may_send = if logout.continuing {
            self.logout_intent.as_ref().is_some_and(|intent| {
                intent.target.session.same_identity(&logout.target.session)
                    && intent.target.cookie.agrees_with(sent)
                    && intent.target.cookie.agrees_with(&logout.request.cookie)
                    && intent.last_sent != ticket
            })
        } else {
            !self.identity_unconfirmed && self.logout_intent.is_none()
        };
        if !may_send
            || !verified
                .as_ref()
                .is_some_and(|verified| verified.session.same_identity(&logout.target.session))
            || logout.target.cookie.conflicts_with(sent)
            || logout.request.cookie.conflicts_with(sent)
        {
            let continuing = logout.continuing;
            self.leave_logout_request(continuing);
            self.begin_read(CheckReason::AfterLogout, actions);
            return;
        }
        // 独立回执归属只看这次实际发送后的代际；先前撤销的 GET 补播资格仍保留。
        logout.saw_other_generation = false;
    }

    fn leave_logout_request(&mut self, unconfirmed: bool) {
        let verified = self.phase.take_verified();
        self.phase = if unconfirmed {
            Phase::LogoutUnconfirmed(verified)
        } else {
            Phase::Checking(verified)
        };
    }

    /// 原意图已见异代时撤销 GET 补播资格，同时保留独立 DELETE 回执的归属。
    pub(super) fn observe_logout_identity(&mut self, conflicts: impl Fn(&Verified) -> bool) {
        if let Phase::LoggingOut(_, logout) = &mut self.phase
            && conflicts(&logout.target)
        {
            logout.saw_other_generation = true;
        }
        if self.logout_intent.as_ref().is_some_and(|intent| conflicts(&intent.target)) {
            self.revoke_logout_read_notice();
        }
    }

    /// 缺少代际证据时只关闭 GET 补播。可信的独立 DELETE 成功仍可消费通知。
    pub(super) fn revoke_logout_read_notice(&mut self) {
        if let Some(intent) = &mut self.logout_intent {
            intent.can_infer_from_read = false;
        }
    }

    /// 用读取登记时的退出票及三个观测点证明通知归属，避免误播到另一会话。
    pub(super) fn logout_notice_from_read(
        &mut self,
        check: &Check,
        sent: &Cookie,
        observed: &Cookie,
    ) -> Option<bool> {
        let intent = self.logout_intent.as_mut()?;
        if !check.origin.as_ref().is_some_and(|origin| origin.same_identity(&intent.target.session))
            || check.logout_ticket != Some(intent.last_sent)
            || !check.request.matches_generation(sent, observed)
            || !(intent.target.cookie.agrees_with(sent) || matches!(sent, Cookie::Missing))
        {
            intent.can_infer_from_read = false;
        }
        Some(intent.can_infer_from_read)
    }

    /// 完整成功或权威 401 才能结束本地退出，后续拒绝不能解除先前未知的写锁。
    pub(super) fn finish_logout(
        &mut self,
        ticket: Ticket,
        sent: &Cookie,
        outcome: LogoutOutcome,
        dispatched: bool,
        actions: &mut Actions,
    ) {
        let Phase::LoggingOut(_, logout) = &self.phase else { return };
        if logout.request.ticket != ticket {
            return;
        }
        if dispatched {
            // 发送前失败保留上次的票，曾撤销的 GET 补播资格不能被新的发送恢复。
            let can_infer_from_read = !logout.saw_other_generation
                && logout.request.cookie.agrees_with(sent)
                && logout.target.cookie.agrees_with(sent);
            if let Some(intent) = &mut self.logout_intent {
                intent.last_sent = ticket;
                intent.can_infer_from_read &= can_infer_from_read;
            } else {
                self.logout_intent = Some(LogoutIntent {
                    target: logout.target.clone(),
                    last_sent: ticket,
                    can_infer_from_read,
                });
            }
        }
        let success = matches!(outcome, LogoutOutcome::Succeeded);
        let unknown = matches!(outcome, LogoutOutcome::Unknown(_));
        // DELETE 正文可能读失败而响应头已清 Cookie；只有稳定读取能补齐通知证据。
        let may_have_cleared_cookie =
            (unknown || success) && matches!(actions.cookie, Cookie::Missing);
        let stale = logout.request.is_stale(sent, &actions.cookie, may_have_cleared_cookie)
            || logout.target.cookie.conflicts_with(sent)
            || (!may_have_cleared_cookie && logout.target.cookie.conflicts_with(&actions.cookie));
        let continuing = logout.continuing;
        let review_after = logout.review_after;
        let was_writable = logout.was_writable;
        // 已核对的原代发送票收到完整成功且清除了 Cookie，可独立于 GET 补播资格结清通知。
        let cleared_by_own_request = success
            && matches!(actions.cookie, Cookie::Missing)
            && logout.request.matches_generation(sent, &logout.target.cookie);
        let notify = (!logout.saw_other_generation || cleared_by_own_request)
            && self.logout_intent.as_ref().is_some_and(|intent| intent.last_sent == ticket);
        let target = Rc::clone(&logout.target.session);
        self.retire_renewal();
        self.leave_logout_request(continuing || unknown);
        if stale {
            self.revoke_logout_read_notice();
            self.begin_read(CheckReason::CookieChanged, actions);
            return;
        }
        match outcome {
            LogoutOutcome::Succeeded => {
                if notify {
                    self.settle_end_notice(&target, actions);
                }
                self.end_session(actions);
                if review_after {
                    self.begin_read(CheckReason::AfterLogout, actions);
                }
            }
            LogoutOutcome::Unknown(error) => {
                self.logout_error = Some(error);
                self.begin_read(CheckReason::AfterLogout, actions);
            }
            LogoutOutcome::Rejected(error) | LogoutOutcome::RateLimited(error) if continuing => {
                self.logout_error = Some(error);
                if review_after {
                    self.begin_read(CheckReason::AfterLogout, actions);
                }
            }
            LogoutOutcome::Rejected(error)
                if was_writable
                    && !review_after
                    && !self.identity_unconfirmed
                    && sent.agrees_with(&actions.cookie) =>
            {
                self.logout_intent = None;
                self.logout_error = Some(error);
                if let Some(verified) = self.phase.take_verified() {
                    self.confirm(verified, actions);
                }
            }
            LogoutOutcome::Rejected(error) | LogoutOutcome::RateLimited(error) => {
                // 已撤销的资格留到复核终结，避免随后把无法归属的 401 当作自然过期。
                self.logout_intent.take_if(|intent| intent.can_infer_from_read);
                self.logout_error = Some(error);
                self.begin_read(CheckReason::AfterLogout, actions);
            }
        }
    }
}
