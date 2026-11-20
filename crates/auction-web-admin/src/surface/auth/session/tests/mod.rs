//! Admin 会话状态机与宿主的测试夹具。

use std::cell::Cell;
use std::future::{pending, ready};
use std::mem::DropGuard;
use std::rc::Rc;

use auction_time::{Duration, Timestamp};
use dioxus::history::MemoryHistory;
use dioxus::prelude::*;
use dioxus::router::{Navigator, RouterConfig};

use super::super::effects::TestSession;
use super::{
    CheckReason, Cookie, Effect, Event, LogoutOutcome, Phase, ReadOutcome, RenewalOutcome, State,
    Ticket, Timer, step,
};
use crate::api::{AdminSession, AdminSessionWriteReview, AdminWriteEndpoint, AdminWriteFailure};
use crate::surface::auctions::draft_page_tests::{
    button, click_button, has_live_text, init_i18n, mount_writer,
};
use crate::surface::pending::use_pending_commands;
use crate::surface::realtime::{TestPage, text_attributes};
use crate::surface::router::{Route, TestNotFoundPage};
use crate::surface::test_events::dispatch_click_event;
use crate::surface::users::AdminUserGeneratedKey;

mod identity;
mod invalidation;
mod logout;
mod pages;
mod projections;
mod sequences;

const NOW: f64 = 1_000.0;

struct SessionFixture {
    session: Rc<AdminSession>,
    cookie: Cookie,
}

impl SessionFixture {
    fn new(token: &str, idle_seconds: i64) -> Self {
        let server_time = Timestamp::parse_from_rfc3339("2026-11-18T00:00:00Z").unwrap();
        let session = Rc::new(AdminSession {
            actor: "operator".to_owned(),
            csrf_token: token.to_owned(),
            expires_at: server_time.checked_add_signed(Duration::seconds(idle_seconds)).unwrap(),
            absolute_expires_at: server_time.checked_add_signed(Duration::hours(8)).unwrap(),
            server_time,
            received_at_millis: 0.0,
        });
        Self { cookie: Cookie::from_session(&session), session }
    }

    fn projection_after(
        &self,
        elapsed_seconds: i64,
        remaining_seconds: i64,
        received_at: f64,
    ) -> Self {
        let mut session = self.session.as_ref().clone();
        session.server_time =
            session.server_time.checked_add_signed(Duration::seconds(elapsed_seconds)).unwrap();
        session.expires_at =
            session.server_time.checked_add_signed(Duration::seconds(remaining_seconds)).unwrap();
        session.received_at_millis = received_at;
        Self { session: Rc::new(session), cookie: self.cookie.clone() }
    }

    fn apply(&self, state: State, event: Event, now: f64) -> (State, Vec<Effect>) {
        step(state, event, self.cookie.clone(), now)
    }

    fn read_ok(&self, ticket: Ticket) -> Event {
        Event::ReadFinished(
            ticket,
            self.cookie.clone(),
            ReadOutcome::Authenticated(Rc::clone(&self.session)),
        )
    }

    fn authenticated(&self) -> State {
        let (state, effects) = self.apply(State::default(), Event::Start, NOW);
        self.apply(state, self.read_ok(read_ticket(&effects)), NOW).0
    }

    fn start_renewal(&self, state: State) -> (State, Ticket) {
        self.start_renewal_at(state, NOW)
    }

    fn start_renewal_at(&self, state: State, now: f64) -> (State, Ticket) {
        let (state, effects) = self.apply(state, Event::Input { on_logout: false }, now);
        let (timer, delay) = armed_timer(&effects, Timer::Renewal);
        let (state, effects) =
            self.apply(state, Event::TimerElapsed(Timer::Renewal, timer), now + f64::from(delay));
        (
            state,
            issued_ticket(&effects, |effect| match effect {
                Effect::Renew(ticket, _) => Some(*ticket),
                _ => None,
            }),
        )
    }
}

fn issued_ticket(effects: &[Effect], gate: impl FnMut(&Effect) -> Option<Ticket>) -> Ticket {
    let tickets: Vec<_> = effects.iter().filter_map(gate).collect();
    assert_eq!(tickets.len(), 1);
    tickets[0]
}

fn read_ticket(effects: &[Effect]) -> Ticket {
    issued_ticket(effects, |effect| match effect {
        Effect::Read(ticket) => Some(*ticket),
        _ => None,
    })
}

fn logout_ticket(effects: &[Effect]) -> Ticket {
    issued_ticket(effects, |effect| match effect {
        Effect::Logout(ticket, _) => Some(*ticket),
        _ => None,
    })
}

fn armed_timer(effects: &[Effect], timer: Timer) -> (Ticket, u32) {
    let timers: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::ArmTimer(kind, ticket, delay) if *kind == timer => Some((*ticket, *delay)),
            _ => None,
        })
        .collect();
    assert_eq!(timers.len(), 1);
    timers[0]
}

fn count_notices(effects: &[Effect]) -> usize {
    effects.iter().filter(|effect| matches!(effect, Effect::BroadcastEnded)).count()
}

fn logged_out() -> State {
    let (state, effects) = step(State::default(), Event::Start, Cookie::Missing, NOW);
    step(
        state,
        Event::ReadFinished(read_ticket(&effects), Cookie::Missing, ReadOutcome::Unauthenticated),
        Cookie::Missing,
        NOW,
    )
    .0
}

#[test]
fn login_receipt_after_broadcast_reads_the_current_cookie_before_opening_writes() {
    let new = SessionFixture::new("new-cookie", 60);
    let mut state = logged_out();
    let login = state.start_login().unwrap();
    assert!(state.view().login_pending);
    assert!(state.start_login().is_none());
    for event in [Event::Broadcast, Event::Visible, Event::Broadcast, Event::CheckRequested] {
        let (next, effects) = step(state, event, Cookie::Missing, NOW);
        assert!(effects.is_empty());
        assert!(next.view().session.is_none());
        assert!(!next.view().logged_out);
        assert!(!next.view().can_write);
        state = next;
    }

    let completed = Event::LoginFinished(login, Ok(Rc::clone(&new.session)));
    let (state, effects) = new.apply(state, completed.clone(), NOW);
    let check = read_ticket(&effects);
    assert_eq!(effects.len(), 1);
    assert!(!state.view().can_write);
    let (state, effects) = new.apply(state, completed, NOW);
    assert!(effects.is_empty());
    let (state, _) = new.apply(state, new.read_ok(check), NOW);
    assert!(state.view().can_write);
    assert_eq!(state.view().session.as_deref(), Some(new.session.as_ref()));
}

#[test]
fn login_completion_with_no_current_session_does_not_repost_or_open_writes() {
    let mut state = logged_out();
    let login = state.start_login().unwrap();
    let (mut state, effects) = step(
        state,
        Event::LoginFinished(login, Err("request_timeout".to_owned())),
        Cookie::Missing,
        NOW,
    );
    assert!(state.view().logged_out);
    assert!(state.view().login_pending);
    assert!(!state.view().can_write);
    assert!(state.start_login().is_none());
    let (state, effects) = step(
        state,
        Event::ReadFinished(read_ticket(&effects), Cookie::Missing, ReadOutcome::Unauthenticated),
        Cookie::Missing,
        NOW,
    );
    assert!(effects.is_empty());
    assert!(state.view().logged_out);
    assert!(!state.view().login_pending);
    assert!(!state.view().can_write);
    assert!(state.view().session.is_none());
}

fn login_after_old_session_expires(old: &SessionFixture) -> (State, Ticket, Ticket) {
    let (state, renewal) = old.start_renewal(old.authenticated());
    let (state, effects) = old.apply(state, Event::Visible, NOW);
    let (mut state, _) = old.apply(
        state,
        Event::ReadFinished(
            read_ticket(&effects),
            old.cookie.clone(),
            ReadOutcome::Unauthenticated,
        ),
        NOW,
    );
    let login = state.start_login().unwrap();
    (state, renewal, login)
}

#[test]
fn old_renewal_401_after_broadcast_does_not_remount_the_logging_in_form() {
    let old = SessionFixture::new("old-cookie", 900);
    let (state, renewal, _) = login_after_old_session_expires(&old);
    let (state, _) = old.apply(state, Event::Broadcast, NOW);
    assert!(!state.view().logged_out);

    let (state, effects) = old.apply(
        state,
        Event::RenewalFinished(renewal, old.cookie.clone(), RenewalOutcome::Unauthenticated),
        NOW,
    );
    assert!(effects.is_empty());
    assert!(state.renewal.is_none());
    assert!(state.view().login_pending);
    assert!(!state.view().logged_out);
    assert!(!state.view().can_write);
}

#[test]
fn old_renewal_401_does_not_replace_the_new_cookie_login_get() {
    let old = SessionFixture::new("old-cookie", 900);
    let new = SessionFixture::new("new-cookie", 60);
    let (state, renewal, login) = login_after_old_session_expires(&old);
    let (state, _) = old.apply(state, Event::Broadcast, NOW);
    let (state, effects) =
        new.apply(state, Event::LoginFinished(login, Ok(Rc::clone(&new.session))), NOW);
    let check = read_ticket(&effects);
    let (state, effects) = new.apply(
        state,
        Event::RenewalFinished(renewal, old.cookie, RenewalOutcome::Unauthenticated),
        NOW,
    );
    assert!(effects.is_empty());
    assert!(state.renewal.is_none());
    assert!(!state.view().can_write);
    assert!(state.check.as_ref().is_some_and(|pending| pending.request.ticket == check));
    let (state, _) = new.apply(state, new.read_ok(check), NOW);
    assert!(state.view().can_write);
}

#[test]
fn discarded_renewal_schedules_new_input_after_logout_read_confirms_first() {
    let current = SessionFixture::new("current-cookie", 60);
    let renewed = current.projection_after(1, 600, NOW);
    let (state, renewal) = current.start_renewal(current.authenticated());
    let (state, effects) = current.apply(state, Event::LogoutClicked, NOW);
    let (state, effects) = current.apply(
        state,
        Event::LogoutFinished(
            logout_ticket(&effects),
            current.cookie.clone(),
            LogoutOutcome::RateLimited("rate_limited".to_owned()),
        ),
        NOW,
    );
    let (state, _) = current.apply(state, current.read_ok(read_ticket(&effects)), NOW);
    let (state, effects) = current.apply(state, Event::Input { on_logout: false }, NOW + 1.0);
    assert!(effects.is_empty());
    assert_eq!(state.activity.latest_input, Some(NOW + 1.0));
    let (state, effects) = current.apply(
        state,
        Event::RenewalFinished(
            renewal,
            current.cookie.clone(),
            RenewalOutcome::Succeeded(renewed.session),
        ),
        NOW + 2.0,
    );
    assert_eq!(state.view().session.as_deref(), Some(current.session.as_ref()));
    let (timer, delay) = armed_timer(&effects, Timer::Renewal);
    let (_, effects) = current.apply(
        state,
        Event::TimerElapsed(Timer::Renewal, timer),
        NOW + 2.0 + f64::from(delay),
    );
    assert!(matches!(effects.as_slice(), [Effect::Renew(ticket, session)] if
        *ticket != renewal && Rc::ptr_eq(session, &current.session)));
}

#[test]
fn late_logout_cookie_removal_keeps_new_login_closed_after_broadcast() {
    let old = SessionFixture::new("old-cookie", 900);
    let new = SessionFixture::new("new-cookie", 60);
    let (tab_a, effects) = old.apply(old.authenticated(), Event::LogoutClicked, NOW);
    let mut tab_b = logged_out();
    let login = tab_b.start_login().unwrap();
    let (tab_b, effects_b) =
        new.apply(tab_b, Event::LoginFinished(login, Ok(Rc::clone(&new.session))), NOW);
    let (tab_b, _) = new.apply(tab_b, new.read_ok(read_ticket(&effects_b)), NOW);
    assert!(tab_b.view().can_write);

    // 浏览器在收到 DELETE 响应时已清除同名 Cookie，外壳只能按当前 Cookie 复核。
    let (_, effects) = step(
        tab_a,
        Event::LogoutFinished(logout_ticket(&effects), old.cookie, LogoutOutcome::Succeeded),
        Cookie::Missing,
        NOW,
    );
    assert_eq!(count_notices(&effects), 1);
    let (tab_b, effects) = step(tab_b, Event::Broadcast, Cookie::Missing, NOW);
    assert!(tab_b.view().session.is_none());
    assert!(!tab_b.view().can_write);
    let (tab_b, _) = step(
        tab_b,
        Event::ReadFinished(read_ticket(&effects), Cookie::Missing, ReadOutcome::Unauthenticated),
        Cookie::Missing,
        NOW,
    );
    assert!(tab_b.view().logged_out);
    assert!(!tab_b.view().can_write);
}

#[test]
fn late_old_401_does_not_end_another_tabs_new_login() {
    let old = SessionFixture::new("old-cookie", 900);
    let new = SessionFixture::new("new-cookie", 60);
    let (tab_a, effects) = old.apply(old.authenticated(), Event::LogoutClicked, NOW);
    let old_epoch = tab_a.view().epoch;
    let logout = logout_ticket(&effects);
    let (tab_a, effects) = old.apply(
        tab_a,
        Event::LogoutFinished(
            logout,
            old.cookie.clone(),
            LogoutOutcome::Unknown("request_timeout".to_owned()),
        ),
        NOW,
    );
    let late_check = read_ticket(&effects);

    let mut tab_b = logged_out();
    let login = tab_b.start_login().unwrap();
    let (tab_b, effects) =
        new.apply(tab_b, Event::LoginFinished(login, Ok(Rc::clone(&new.session))), NOW);
    let (tab_b, _) = new.apply(tab_b, new.read_ok(read_ticket(&effects)), NOW);
    assert!(tab_b.view().can_write);

    let (tab_a, effects) = new.apply(
        tab_a,
        Event::ReadFinished(late_check, old.cookie.clone(), ReadOutcome::Unauthenticated),
        NOW,
    );
    assert_eq!(count_notices(&effects), 0);
    assert_eq!(tab_a.view().session.as_deref(), Some(old.session.as_ref()));
    assert_eq!(tab_a.view().epoch, old_epoch);
    assert!(tab_a.view().key_visibility.hidden);
    assert!(!tab_a.view().can_write);
    assert!(tab_b.view().can_write);
    assert_eq!(tab_b.view().session.as_deref(), Some(new.session.as_ref()));
    let (tab_a, effects) = new.apply(tab_a, new.read_ok(read_ticket(&effects)), NOW);
    assert!(tab_a.view().logged_out);
    assert!(tab_a.view().epoch > old_epoch);
    assert_eq!(count_notices(&effects), 0);
    let (tab_a, effects) = new.apply(tab_a, Event::Visible, NOW);
    let (tab_a, _) = new.apply(tab_a, new.read_ok(read_ticket(&effects)), NOW);
    assert!(tab_a.view().can_write);
    assert_eq!(tab_a.view().session.as_deref(), Some(new.session.as_ref()));

    // 固定通知可能在新登录后才送达。接收方须先隐藏，再凭当前代际恢复一棵新宿主。
    let epoch = tab_b.view().epoch;
    let (tab_b, effects) = new.apply(tab_b, Event::Broadcast, NOW);
    assert!(tab_b.view().session.is_none());
    assert!(!tab_b.view().can_write);
    let (tab_b, _) = new.apply(tab_b, new.read_ok(read_ticket(&effects)), NOW);
    assert!(tab_b.view().can_write);
    assert!(tab_b.view().epoch > epoch);
    assert_eq!(tab_b.view().session.as_deref(), Some(new.session.as_ref()));
}

#[test]
fn broadcast_then_old_get_success_cannot_interrupt_delete_success() {
    let current = SessionFixture::new("current-cookie", 900);
    let (state, effects) = current.apply(current.authenticated(), Event::Visible, NOW);
    let old_check = read_ticket(&effects);
    assert!(state.view().can_write);
    let (state, effects) = current.apply(state, Event::LogoutClicked, NOW);
    let logout = logout_ticket(&effects);
    assert!(effects.iter().any(|effect| matches!(effect, Effect::CancelRead)));
    let (state, effects) = current.apply(state, Event::Broadcast, NOW);
    assert!(state.view().logging_out);
    assert!(!state.view().can_write);
    assert!(state.view().session.is_none());
    assert!(effects.is_empty());

    let (state, effects) = current.apply(state, current.read_ok(old_check), NOW);
    assert!(state.view().logging_out);
    assert!(!state.view().can_write);
    assert!(effects.is_empty());

    let succeeded = Event::LogoutFinished(logout, current.cookie, LogoutOutcome::Succeeded);
    let (state, effects) = step(state, succeeded.clone(), Cookie::Missing, NOW);
    // 对端已报告同一次结束，迟到的本地成功只结清退出。
    assert_eq!(count_notices(&effects), 0);
    assert!(!state.view().can_write);
    assert!(!state.view().logging_out);
    let check = read_ticket(&effects);
    let (state, effects) = step(state, succeeded, Cookie::Missing, NOW);
    assert!(effects.is_empty());
    assert!(!state.view().can_write);
    let (state, effects) = step(
        state,
        Event::ReadFinished(check, Cookie::Missing, ReadOutcome::Unauthenticated),
        Cookie::Missing,
        NOW,
    );
    assert!(state.view().logged_out);
    assert!(state.view().session.is_none());
    assert!(effects.is_empty());
}

#[test]
fn renewal_timeout_starts_one_read_when_routine_503_closed_writes() {
    let current = SessionFixture::new("current-cookie", 900);
    let (state, renewal) = current.start_renewal(current.authenticated());
    let (state, effects) = current.apply(state, Event::Visible, NOW);
    let (state, effects) = current.apply(
        state,
        Event::ReadFinished(
            read_ticket(&effects),
            current.cookie.clone(),
            ReadOutcome::Failed("storage_unavailable".to_owned()),
        ),
        NOW,
    );
    assert!(!state.view().can_write);
    assert!(state.view().session.is_some());
    assert_eq!(armed_timer(&effects, Timer::Check).1, 30_000);

    let timed_out =
        Event::RenewalFinished(renewal, current.cookie.clone(), RenewalOutcome::Unknown);
    let (state, effects) = current.apply(state, timed_out.clone(), NOW);
    let read = read_ticket(&effects);
    assert!(!state.view().can_write);
    assert!(effects.iter().any(|effect| matches!(effect, Effect::CancelTimer(Timer::Check))));
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            Effect::Renew(..) | Effect::ArmTimer(Timer::Renewal, ..)
        ))
    );
    let (state, effects) = current.apply(state, timed_out, NOW);
    assert!(effects.is_empty());
    let (state, effects) = current.apply(state, current.read_ok(read), NOW);
    assert!(state.view().can_write);
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            Effect::Renew(..) | Effect::ArmTimer(Timer::Renewal, ..)
        ))
    );
}

#[test]
fn rate_limit_preserves_input_recorded_after_post_until_retry_after() {
    let current = SessionFixture::new("current-cookie", 60);
    let (state, renewal) = current.start_renewal(current.authenticated());
    let (state, effects) = current.apply(state, Event::Input { on_logout: false }, NOW + 10.0);
    assert!(effects.is_empty());

    let (state, effects) = current.apply(
        state,
        Event::RenewalFinished(
            renewal,
            current.cookie.clone(),
            RenewalOutcome::RateLimited(Some(10_000)),
        ),
        NOW + 20.0,
    );
    let (timer, delay) = armed_timer(&effects, Timer::Renewal);
    assert_eq!(delay, 10_000);
    assert_eq!(state.activity.latest_input, Some(NOW + 10.0));

    let due_at = NOW + 20.0 + f64::from(delay);
    let (state, effects) = current.apply(state, Event::TimerElapsed(Timer::Renewal, timer), due_at);
    let next = issued_ticket(&effects, |effect| match effect {
        Effect::Renew(ticket, _) => Some(*ticket),
        _ => None,
    });
    assert_ne!(renewal, next);
    assert!(state.activity.latest_input.is_none());
    let (state, effects) = current.apply(
        state,
        Event::RenewalFinished(
            next,
            current.cookie.clone(),
            RenewalOutcome::RateLimited(Some(10_000)),
        ),
        due_at + 1.0,
    );
    assert!(state.renewal_timer.is_none());
    assert!(effects.is_empty());
}

#[test]
fn rate_limit_drops_fresh_input_if_retry_cannot_reach_deadline() {
    let current = SessionFixture::new("current-cookie", 60);
    let (state, renewal) = current.start_renewal(current.authenticated());
    let (state, _) = current.apply(state, Event::Input { on_logout: false }, NOW + 10.0);
    let (state, effects) = current.apply(
        state,
        Event::RenewalFinished(
            renewal,
            current.cookie.clone(),
            RenewalOutcome::RateLimited(Some(58_980)),
        ),
        NOW + 20.0,
    );
    assert!(effects.is_empty());
    assert!(state.activity.latest_input.is_none());
    assert!(state.check_timer.is_some());
}

#[test]
fn rate_limit_keeps_new_input_for_a_ninety_second_backoff_before_expiration() {
    let current = SessionFixture::new("current-cookie", 600);
    let (state, renewal) = current.start_renewal(current.authenticated());
    let (state, _) = current.apply(state, Event::Input { on_logout: false }, NOW + 10.0);
    let (state, effects) = current.apply(
        state,
        Event::RenewalFinished(
            renewal,
            current.cookie.clone(),
            RenewalOutcome::RateLimited(Some(90_000)),
        ),
        NOW + 20.0,
    );
    let (timer, delay) = armed_timer(&effects, Timer::Renewal);
    assert_eq!(delay, 90_000);
    let (state, effects) = current.apply(
        state,
        Event::TimerElapsed(Timer::Renewal, timer),
        NOW + 20.0 + f64::from(delay),
    );
    let next = issued_ticket(&effects, |effect| match effect {
        Effect::Renew(ticket, _) => Some(*ticket),
        _ => None,
    });
    assert_ne!(next, renewal);
    assert!(state.activity.latest_input.is_none());
}

#[test]
fn logout_gesture_never_renews_and_logout_429_stays_read_only_until_checked() {
    let current = SessionFixture::new("current-cookie", 900);
    let (state, effects) =
        current.apply(current.authenticated(), Event::Input { on_logout: true }, NOW);
    assert!(effects.is_empty());
    assert!(state.activity.latest_input.is_none());
    let (state, effects) = current.apply(state, Event::Input { on_logout: false }, NOW + 1.0);
    let (timer, delay) = armed_timer(&effects, Timer::Renewal);
    assert_eq!(delay, 0);
    let (state, effects) = current.apply(state, Event::LogoutClicked, NOW + 1.0);
    assert!(effects.iter().any(|effect| matches!(effect, Effect::CancelTimer(Timer::Renewal))));
    let logout = logout_ticket(&effects);
    let (state, effects) =
        current.apply(state, Event::TimerElapsed(Timer::Renewal, timer), NOW + 1.0);
    assert!(effects.is_empty());
    let (state, effects) = current.apply(state, Event::Input { on_logout: false }, NOW + 2.0);
    assert!(effects.is_empty());
    assert!(!state.view().can_write);

    let (state, effects) = current.apply(
        state,
        Event::LogoutFinished(
            logout,
            current.cookie.clone(),
            LogoutOutcome::RateLimited("rate_limited".to_owned()),
        ),
        NOW + 3.0,
    );
    assert!(!state.view().can_write);
    assert!(!state.view().logging_out);
    assert!(state.check.as_ref().is_some_and(|check| check.reason == CheckReason::AfterLogout));
    let (state, _) = current.apply(state, current.read_ok(read_ticket(&effects)), NOW + 4.0);
    assert!(state.view().can_write);
    assert_eq!(state.view().logout_error.as_deref(), Some("rate_limited"));
}

#[test]
fn stale_successful_logout_only_checks_the_replacement_cookie() {
    let previous = SessionFixture::new("old-cookie", 900);
    let current = SessionFixture::new("new-cookie", 900);
    let (state, effects) = previous.apply(previous.authenticated(), Event::LogoutClicked, NOW);
    let (state, effects) = current.apply(
        state,
        Event::LogoutFinished(
            logout_ticket(&effects),
            previous.cookie.clone(),
            LogoutOutcome::Succeeded,
        ),
        NOW,
    );
    assert_eq!(count_notices(&effects), 0);
    assert_eq!(state.view().session.as_deref(), Some(previous.session.as_ref()));
    assert!(state.view().key_visibility.hidden);
    assert!(!state.view().can_write);
    let epoch = state.view().epoch;
    let (state, effects) = current.apply(state, current.read_ok(read_ticket(&effects)), NOW);
    assert!(state.view().logged_out);
    assert!(state.view().epoch > epoch);
    assert_eq!(count_notices(&effects), 0);
}

#[test]
fn request_dispatched_after_cookie_change_only_rereads_current_cookie() {
    let previous = SessionFixture::new("old-cookie", 900);
    let current = SessionFixture::new("new-cookie", 900);
    let (state, effects) = previous.apply(previous.authenticated(), Event::Visible, NOW);
    let ticket = read_ticket(&effects);
    assert!(state.view().can_write);
    assert!(!state.view().key_visibility.hidden);
    let (state, effects) = current.apply(state, Event::ReadSending(ticket), NOW);
    assert_review_hides_previous_session(&state, &previous);
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Read(_))));
    assert!(state.has_read(ticket));
    assert_replacement_read_ends_previous_session(state, &current, ticket);
}

#[test]
fn request_dispatched_before_cookie_change_rereads_after_old_response() {
    let previous = SessionFixture::new("old-cookie", 900);
    let current = SessionFixture::new("new-cookie", 900);
    let (state, effects) = previous.apply(previous.authenticated(), Event::Visible, NOW);
    let ticket = read_ticket(&effects);
    let (state, effects) = previous.apply(state, Event::ReadSending(ticket), NOW);
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Read(_))));
    assert!(state.view().can_write);
    assert!(!state.view().key_visibility.hidden);

    let (state, effects) = current.apply(state, previous.read_ok(ticket), NOW);
    assert_review_hides_previous_session(&state, &previous);
    assert_eq!(count_notices(&effects), 0);
    let replacement = read_ticket(&effects);
    assert_ne!(replacement, ticket);
    let (state, effects) = current.apply(state, Event::ReadSending(replacement), NOW);
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Read(_))));
    assert_replacement_read_ends_previous_session(state, &current, replacement);
}

fn assert_review_hides_previous_session(state: &State, previous: &SessionFixture) {
    assert_eq!(state.view().session.as_deref(), Some(previous.session.as_ref()));
    assert!(state.view().key_visibility.hidden);
    assert!(!state.view().can_write);
}

fn assert_replacement_read_ends_previous_session(
    state: State,
    current: &SessionFixture,
    ticket: Ticket,
) {
    let epoch = state.view().epoch;
    let (state, effects) = current.apply(state, current.read_ok(ticket), NOW);
    assert!(state.view().logged_out);
    assert!(state.view().epoch > epoch);
    assert!(state.view().session.is_none());
    assert!(!state.view().can_write);
    assert_eq!(count_notices(&effects), 0);
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Read(_))));

    let (state, effects) = current.apply(state, Event::Visible, NOW);
    let ticket = read_ticket(&effects);
    let (state, _) = current.apply(state, Event::ReadSending(ticket), NOW);
    let (state, _) = current.apply(state, current.read_ok(ticket), NOW);
    assert_eq!(state.view().session.as_deref(), Some(current.session.as_ref()));
    assert!(state.view().can_write);
}

#[test]
fn routine_and_explicit_reads_only_keep_writes_for_the_confirmed_cookie() {
    let current = SessionFixture::new("current-cookie", 900);
    let other = SessionFixture::new("other-cookie", 900);
    let (state, _) = current.apply(current.authenticated(), Event::Visible, NOW);
    assert!(state.view().can_write);
    let (state, effects) = current.apply(state, Event::CheckRequested, NOW);
    assert!(!state.view().can_write);
    assert!(effects.iter().any(|effect| matches!(effect, Effect::CancelRead)));
    let (state, _) = current.apply(state, current.read_ok(read_ticket(&effects)), NOW);
    assert!(state.view().can_write);

    let (state, effects) = other.apply(state, Event::Visible, NOW);
    assert!(!state.view().can_write);
    let (state, effects) = other.apply(state, other.read_ok(read_ticket(&effects)), NOW);
    assert!(state.view().logged_out);
    assert!(state.view().session.is_none());
    assert_eq!(count_notices(&effects), 0);
}

#[test]
fn confirmed_expiration_unmounts_and_broadcasts_only_a_verified_generation() {
    let current = SessionFixture::new("current-cookie", 900);
    for (cookie, expected_notices) in
        [(current.cookie.clone(), 1), (Cookie::Missing, 1), (Cookie::Unavailable, 0)]
    {
        let (state, effects) = step(current.authenticated(), Event::Visible, cookie.clone(), NOW);
        let (state, effects) = step(
            state,
            Event::ReadFinished(
                read_ticket(&effects),
                cookie.clone(),
                ReadOutcome::Unauthenticated,
            ),
            cookie,
            NOW,
        );
        assert!(matches!(state.phase, Phase::LoggedOut));
        assert!(!state.view().can_write);
        assert!(state.view().session.is_none());
        assert_eq!(count_notices(&effects), expected_notices);
    }
}

#[derive(Clone, Copy)]
enum SessionEnd {
    Logout,
    IdleExpiration,
    Broadcast,
}

#[derive(Default)]
struct CommandRequests {
    sent: Cell<usize>,
    cancelled: Cell<bool>,
    source_unmounted: Rc<Cell<bool>>,
    navigation: Cell<Option<Navigator>>,
}

#[component]
fn AuthenticatedCommands() -> Element {
    let reload = use_signal(|| 0);
    let commands = use_pending_commands(reload, |_| true);
    let review = use_context::<AdminSessionWriteReview>();
    let session = use_context::<AdminSession>();
    let requests = use_context::<Rc<CommandRequests>>();
    let navigation = navigator();
    requests.navigation.set(Some(navigation));
    let delayed = requests.clone();
    rsx! {
        button { onclick: move |_| {
            assert!(commands.submit_with_receipt(
                "user:fixture".to_owned(), "测试生成 Key".to_owned(),
                |_| ready(Ok::<_, AdminWriteFailure>("仅供会话测试的一次性 Key".to_owned())),
                |key, _| AdminUserGeneratedKey::delivery(Some(key), false),
                |_| {},
            ));
        }, {"生成测试 Key".to_owned()} }
        button { onclick: move |_| {
            assert!(commands.submit_with_receipt(
                "user:second".to_owned(), "生成第二张 Key 回执".to_owned(),
                |_| ready(Ok::<_, AdminWriteFailure>("第二张回执的测试 Key".to_owned())),
                |key, _| AdminUserGeneratedKey::delivery(Some(key), false),
                |_| {},
            ));
        }, {"生成第二张测试 Key".to_owned()} }
        button { onclick: move |_| {
            let replacement = SessionFixture::new("replacement-cookie", 900);
            review.observe.call((review.epoch, replacement.session.as_ref().clone()));
        }, {"写前复核发现换代".to_owned()} }
        button { onclick: move |_| {
            review.invalidate.call((review.epoch, session.clone()));
        }, {"业务请求收到失效证据".to_owned()} }
        button { onclick: move |_| {
            let _ = navigation.push(Route::Status {});
        }, {"离开命令页".to_owned()} }
        button { onclick: move |_| {
            let requests = requests.clone();
            assert!(commands.submit(
                "alert:fixture".to_owned(), "测试静默".to_owned(), move |_| {
                    requests.sent.set(requests.sent.get() + 1);
                    let reply = AdminWriteFailure::from_test_http(
                        AdminWriteEndpoint::General,
                        503,
                        r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                    );
                    ready(Err::<(), _>(reply))
                },
                |()| panic!("故障替身没有返回成功回执"), |_| {},
            ));
        }, {"发送会话命令".to_owned()} }
        button { onclick: move |_| {
            let delayed = delayed.clone();
            assert!(commands.submit(
                "world:fixture".to_owned(), "测试世界命令".to_owned(), move |_| {
                    let delayed = delayed.clone();
                    async move {
                        delayed.sent.set(delayed.sent.get() + 1);
                        let _request = DropGuard::new(delayed, |requests| {
                            requests.cancelled.set(true);
                        });
                        pending::<Result<(), AdminWriteFailure>>().await
                    }
                },
                |()| panic!("已结束的会话不能交付旧命令"), |_| {},
            ));
        }, {"保持命令在途".to_owned()} }
    }
}

fn command_fixture_route() -> Route {
    Route::NotFound { segments: vec!["test-authenticated-commands".to_owned()] }
}

fn fixture_outlet(segments: Vec<String>) -> Element {
    assert_eq!(segments, ["test-authenticated-commands"]);
    rsx! { AuthenticatedCommands {} }
}

fn end_fixture_session(fixture: &SessionFixture, state: State, end: SessionEnd) -> State {
    match end {
        SessionEnd::Logout => {
            let (state, effects) = fixture.apply(state, Event::LogoutClicked, NOW);
            fixture
                .apply(
                    state,
                    Event::LogoutFinished(
                        logout_ticket(&effects),
                        fixture.cookie.clone(),
                        LogoutOutcome::Succeeded,
                    ),
                    NOW,
                )
                .0
        }
        SessionEnd::IdleExpiration => {
            let ticket = state.check_timer.unwrap();
            let (state, effects) =
                fixture.apply(state, Event::TimerElapsed(Timer::Check, ticket), NOW + 900_000.0);
            fixture
                .apply(
                    state,
                    Event::ReadFinished(
                        read_ticket(&effects),
                        fixture.cookie.clone(),
                        ReadOutcome::Unauthenticated,
                    ),
                    NOW + 900_000.0,
                )
                .0
        }
        SessionEnd::Broadcast => fixture.apply(state, Event::Broadcast, NOW).0,
    }
}

fn mount_session_commands(end: SessionEnd) -> (TestPage, Rc<CommandRequests>) {
    let requests = Rc::new(CommandRequests::default());
    let (page, _) = mount_writer({
        let requests = requests.clone();
        let fixture = Rc::new(SessionFixture::new("current-cookie", 900));
        move || {
            init_i18n();
            use_context_provider(|| requests.clone());
            use_context_provider(|| TestNotFoundPage {
                render: fixture_outlet,
                unmounted: requests.source_unmounted.clone(),
            });
            use_context_provider(|| {
                Rc::new(MemoryHistory::with_initial_path(command_fixture_route()))
                    as Rc<dyn History>
            });
            let mut state = use_signal(|| fixture.authenticated());
            let mut view = use_signal(|| state.peek().view());
            let mut check = use_signal(|| None);
            let events = fixture.clone();
            let dispatch = use_callback(move |event| {
                let before = state.peek().clone();
                let next = if matches!(event, Event::LogoutClicked) {
                    end_fixture_session(&events, before, SessionEnd::Logout)
                } else {
                    let (next, effects) = events.apply(before, event, NOW);
                    if effects.iter().any(|effect| matches!(effect, Effect::Read(_))) {
                        check.set(Some(read_ticket(&effects)));
                    }
                    next
                };
                view.set(next.view());
                state.set(next);
            });
            use_context_provider(|| TestSession(view, dispatch));
            let finish = fixture.clone();
            let end_fixture = fixture.clone();
            let routes = requests.clone();
            rsx! {
                button { onclick: move |_| routes.navigation.get().unwrap().go_back(), {"返回命令页".to_owned()} }
                button { onclick: move |_| dispatch.call(Event::CheckRequested), {"复核测试会话".to_owned()} }
                button { onclick: move |_| {
                    dispatch.call(finish.read_ok(check().unwrap()));
                }, {"复核通过".to_owned()} }
                button { onclick: move |_| {
                    let next = end_fixture_session(&end_fixture, state.peek().clone(), end);
                    view.set(next.view());
                    state.set(next);
                }, {"结束测试会话".to_owned()} }
                button { onclick: move |_| {
                    let next = SessionFixture::new("next-cookie", 900).authenticated();
                    view.set(next.view());
                    state.set(next);
                }, {"开始新会话".to_owned()} }
                Router::<Route> { config: move |_| RouterConfig::<Route>::default().on_update(|router| {
                    matches!(router.current(), Route::Dashboard {}).then(|| command_fixture_route().into())
                }) }
            }
        }
    });
    (page, requests)
}

#[test]
fn pending_commands_survive_session_review_but_cannot_retry_until_confirmed() {
    let (mut page, requests) = mount_session_commands(SessionEnd::Logout);
    click_button(&mut page, "发送会话命令");
    click_button(&mut page, "复核测试会话");
    let (retry, disabled) = button(&page, "原样重试").unwrap();
    assert!(disabled);
    page.dispatch("click", retry, "");
    assert_eq!(requests.sent.get(), 1);
    click_button(&mut page, "复核通过");
    click_button(&mut page, "原样重试");
    assert_eq!(requests.sent.get(), 2);
}

#[test]
fn session_shell_hides_open_key_and_retains_receipt_and_command_until_reconfirmed() {
    for evidence in ["写前复核发现换代", "业务请求收到失效证据"] {
        let (mut page, requests) = mount_session_commands(SessionEnd::IdleExpiration);
        click_button(&mut page, "发送会话命令");
        click_button(&mut page, "生成测试 Key");
        click_button(&mut page, "查看生成的 Key");
        let key = "仅供会话测试的一次性 Key";
        assert!(has_live_text(&page, key));

        click_button(&mut page, evidence);
        assert!(!has_live_text(&page, key));
        assert!(button(&page, "查看生成的 Key").unwrap().1);
        assert!(button(&page, "原样重试").unwrap().1);
        assert!(!requests.source_unmounted.get());
        assert_eq!(requests.sent.get(), 1);

        click_button(&mut page, "复核通过");
        assert!(!has_live_text(&page, key));
        assert!(!button(&page, "查看生成的 Key").unwrap().1);
        click_button(&mut page, "查看生成的 Key");
        assert!(has_live_text(&page, key));
        click_button(&mut page, "结束测试会话");
        assert!(!has_live_text(&page, key));
        assert!(button(&page, "查看生成的 Key").is_none());
        assert!(button(&page, "原样重试").is_none());
    }
}

#[test]
fn session_shell_ignores_the_previous_key_dialogs_late_close() {
    let (mut page, _) = mount_session_commands(SessionEnd::Logout);
    click_button(&mut page, "生成测试 Key");
    click_button(&mut page, "查看生成的 Key");
    let first = "仅供会话测试的一次性 Key";
    let second = "第二张回执的测试 Key";
    assert!(has_live_text(&page, first));
    let (old_close, _) = button(&page, "关闭").unwrap();
    let edits = click_button(&mut page, "生成第二张测试 Key");
    let second_button = text_attributes(&edits, "id")
        .find_map(|(id, value)| value.starts_with("admin-command-receipt-").then_some(id))
        .expect("第二张回执有独立的查看按钮");

    // 同一渲染批次中，打开第二张后仍能收到第一张对话框创建的关闭动作。
    dispatch_click_event(page.dom(), second_button);
    page.dispatch("click", old_close, "");
    assert!(!has_live_text(&page, first));
    assert!(has_live_text(&page, second));
    assert!(button(&page, "关闭").is_some());

    // 关闭当前对话框只消费第二张，随后仍能打开并确认第一张。
    click_button(&mut page, "关闭");
    click_button(&mut page, "查看生成的 Key");
    assert!(has_live_text(&page, first));
    click_button(&mut page, "关闭");
    assert!(button(&page, "查看生成的 Key").is_none());
}

#[test]
fn ending_the_session_drops_unknown_records_and_cancels_only_its_waiting_requests() {
    for end in [SessionEnd::Logout, SessionEnd::IdleExpiration, SessionEnd::Broadcast] {
        let (mut page, requests) = mount_session_commands(end);
        click_button(&mut page, "发送会话命令");
        click_button(&mut page, "保持命令在途");
        assert!(button(&page, "原样重试").is_some());
        assert!(!requests.cancelled.get());
        click_button(
            &mut page,
            if matches!(end, SessionEnd::Logout) { "退出登录" } else { "结束测试会话" },
        );
        assert!(requests.cancelled.get());
        assert!(button(&page, "原样重试").is_none());
        click_button(&mut page, "开始新会话");
        assert!(button(&page, "原样重试").is_none());
        assert_eq!(requests.sent.get(), 2);
        click_button(&mut page, "发送会话命令");
        assert_eq!(requests.sent.get(), 3);
    }
}

#[test]
fn authenticated_outlet_navigation_keeps_the_same_command_host() {
    let (mut page, requests) = mount_session_commands(SessionEnd::Logout);
    click_button(&mut page, "发送会话命令");
    click_button(&mut page, "保持命令在途");
    assert!(!requests.source_unmounted.get());
    click_button(&mut page, "离开命令页");
    assert!(requests.source_unmounted.get());
    assert!(has_live_text(&page, "服务状态"));
    assert!(button(&page, "发送会话命令").is_none());
    assert!(!requests.cancelled.get());
    assert!(button(&page, "原样重试").is_some());
    click_button(&mut page, "返回命令页");
    assert!(button(&page, "发送会话命令").is_some());
    assert!(button(&page, "原样重试").is_some());
    assert!(!requests.cancelled.get());
}
