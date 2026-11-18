use std::rc::Rc;

use auction_time::{Duration, Timestamp};

use super::{
    CheckReason, Cookie, Effect, Event, LogoutOutcome, Phase, ReadOutcome, RenewalOutcome, State,
    Ticket, Timer, step,
};
use crate::api::AdminSession;

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

#[test]
fn late_old_401_does_not_end_another_tabs_new_login() {
    let old = SessionFixture::new("old-cookie", 900);
    let new = SessionFixture::new("new-cookie", 60);
    let (tab_a, effects) = old.apply(old.authenticated(), Event::LogoutClicked, NOW);
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

    let (tab_b, effects) = step(State::default(), Event::Start, Cookie::Missing, NOW);
    let (tab_b, _) = step(
        tab_b,
        Event::ReadFinished(read_ticket(&effects), Cookie::Missing, ReadOutcome::Unauthenticated),
        Cookie::Missing,
        NOW,
    );
    let (tab_b, effects) = new.apply(tab_b, Event::LoginCompleted(Rc::clone(&new.session)), NOW);
    let (tab_b, _) = new.apply(tab_b, new.read_ok(read_ticket(&effects)), NOW);
    assert!(tab_b.view().can_write);

    let (tab_a, effects) = new.apply(
        tab_a,
        Event::ReadFinished(late_check, old.cookie, ReadOutcome::Unauthenticated),
        NOW,
    );
    assert_eq!(count_notices(&effects), 0);
    assert!(tab_a.view().session.is_none());
    assert!(!tab_a.view().can_write);
    assert!(tab_b.view().can_write);
    assert_eq!(tab_b.view().session.as_deref(), Some(new.session.as_ref()));
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
    assert_eq!(count_notices(&effects), 1);
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
        Event::LogoutFinished(logout_ticket(&effects), previous.cookie, LogoutOutcome::Succeeded),
        NOW,
    );
    assert_eq!(count_notices(&effects), 0);
    assert!(state.view().session.is_none());
    assert!(!state.view().can_write);
    let (state, _) = current.apply(state, current.read_ok(read_ticket(&effects)), NOW);
    assert_eq!(state.view().session.as_deref(), Some(current.session.as_ref()));
}

#[test]
fn request_dispatched_after_cookie_change_only_rereads_current_cookie() {
    let previous = SessionFixture::new("old-cookie", 900);
    let current = SessionFixture::new("new-cookie", 900);
    let (state, effects) = previous.apply(previous.authenticated(), Event::Visible, NOW);
    let (state, effects) = current.apply(state, current.read_ok(read_ticket(&effects)), NOW);
    assert!(state.view().session.is_none());
    assert!(!state.view().can_write);
    assert_eq!(count_notices(&effects), 0);
    let (state, _) = current.apply(state, current.read_ok(read_ticket(&effects)), NOW);
    assert_eq!(state.view().session.as_deref(), Some(current.session.as_ref()));
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
