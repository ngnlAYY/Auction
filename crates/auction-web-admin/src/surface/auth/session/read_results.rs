//! 会话读取结果的归属、投影校验与过期通知。

use super::deadlines::is_newer_projection;
use super::notices::cookie_can_end_session;
use super::transition::Actions;
use super::{CheckReason, Cookie, Phase, ReadOutcome, State, Ticket, Timer, Verified};
use crate::surface::auth::activity::RenewalTiming;

const CHECK_RETRY_MILLIS: u32 = 30_000;

impl State {
    /// 用当前票的权威读取确认身份，过时的读取与冲突正文不能恢复授权。
    pub(super) fn finish_read(
        &mut self,
        ticket: Ticket,
        sent: &Cookie,
        outcome: ReadOutcome,
        actions: &mut Actions,
    ) {
        let Some(check) = self.check.take_if(|check| check.request.ticket == ticket) else {
            return;
        };
        let needs_generation = self.keys_hidden();
        if let ReadOutcome::Authenticated(session) = &outcome {
            // 当前票的正文也是证据。Cookie 变动只决定回执能否采用，不能抹掉已见过的异代。
            self.observe_session_body(check.origin.as_deref(), session, actions);
        }
        // 本页登录优先于被动探测。探测只可给旧身份留下证据，登录结论仍由专属 GET 决定。
        let discovering = check.reason == CheckReason::Discover;
        if discovering && self.login.is_some() {
            return;
        }
        let logout_notice = self.logout_notice_from_read(&check, sent, &actions.cookie);
        if check.request.is_stale(sent, &actions.cookie, false) {
            if !discovering {
                let reason = if check.reason == CheckReason::Login {
                    CheckReason::Login
                } else {
                    CheckReason::CookieChanged
                };
                self.begin_read(reason, actions);
            }
            return;
        }
        match outcome {
            ReadOutcome::Authenticated(session) => {
                let mut verified = Verified::new(session);
                if verified.cookie.conflicts_with(&check.request.cookie)
                    || verified.cookie.conflicts_with(sent)
                    || verified.cookie.conflicts_with(&actions.cookie)
                {
                    self.revoke_logout_read_notice();
                    if !discovering {
                        self.fail_read("invalid_response".to_owned(), actions);
                    }
                    return;
                }
                let previous = self
                    .phase
                    .verified()
                    .or_else(|| self.logout_intent.as_ref().map(|intent| &intent.target));
                if needs_generation && !check.request.matches_generation(sent, &actions.cookie) {
                    self.fail_read("invalid_response".to_owned(), actions);
                    return;
                }
                if previous
                    .is_some_and(|previous| !previous.session.same_identity(&verified.session))
                {
                    self.clear_pending_end_notices();
                    self.end_session(actions);
                    return;
                }
                if matches!(self.phase, Phase::LogoutUnconfirmed(_)) {
                    self.identity_unconfirmed = false;
                    self.saw_other_generation = false;
                    // 只有操作员明确选择留在后台的同代读取能结束退出锁定。
                    if check.reason != CheckReason::StayAfterLogout {
                        self.clear_pending_end_notices();
                        if self.phase.verified().is_none() {
                            self.phase = Phase::LogoutUnconfirmed(Some(verified));
                        }
                        return;
                    }
                    self.logout_error = None;
                }
                if discovering {
                    self.view_epoch += 1;
                }
                self.merge_buffered_renewal(&mut verified, sent, actions);
                self.confirm(verified, actions);
            }
            ReadOutcome::Unauthenticated => {
                if let Some(origin) = check.origin
                    && check.request.matches_generation(sent, &actions.cookie)
                    && (logout_notice == Some(true)
                        || self.has_end_evidence(&origin, sent)
                        || (logout_notice.is_none()
                            && self
                                .phase
                                .verified()
                                .is_some_and(|verified| verified.session.same_identity(&origin))
                            && cookie_can_end_session(&origin, sent, self.saw_other_generation)))
                {
                    self.settle_end_notice(&origin, actions);
                }
                if !discovering {
                    self.end_session(actions);
                }
            }
            ReadOutcome::Failed(error) => {
                if !discovering {
                    self.fail_read(error, actions);
                }
            }
        }
    }

    fn fail_read(&mut self, error: String, actions: &mut Actions) {
        if !self.logout_unconfirmed() {
            // 临时故障关闭写门禁，但仍保留已确认宿主的草稿和待对账命令。
            self.phase = Phase::Unavailable(self.phase.take_verified(), error);
            self.cancel_timer(Timer::Renewal, actions);
            self.arm_timer(Timer::Check, CHECK_RETRY_MILLIS, actions);
        }
    }

    fn merge_buffered_renewal(
        &mut self,
        verified: &mut Verified,
        sent: &Cookie,
        actions: &Actions,
    ) {
        if let Some(pending) = self.buffered_renewal.take()
            && pending.cookie.agrees_with(sent)
            && pending.cookie.agrees_with(&actions.cookie)
            && pending.session.same_identity(&verified.session)
        {
            self.activity.timing = Some(RenewalTiming::from_session(&pending.session));
            self.activity.retry_not_before = 0.0;
            if is_newer_projection(&pending.session, &verified.session) {
                // 截止和单调计时基准必须来自同一份回执。
                verified.session = pending.session;
            }
        }
    }
}
