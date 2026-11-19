use std::rc::Rc;

use super::{NOW, SessionFixture, armed_timer, read_ticket};
use crate::surface::auth::session::{
    Cookie, Effect, Event, ReadOutcome, RenewalOutcome, State, Ticket, Timer, step,
};

#[test]
fn late_get_keeps_renewed_deadline_and_its_clock() {
    let current = SessionFixture::new("current-cookie", 60);
    // GET 先读取旧截止，但其响应取时可能晚于续期响应的服务端时刻。
    let read_snapshot = current.projection_after(57, 3, 59_000.0);
    let renewed = current.projection_after(56, 60, 57_000.0);
    let (state, effects) = current.apply(current.authenticated(), Event::Visible, 55_000.0);
    let check = read_ticket(&effects);
    let (state, renewal) = current.start_renewal_at(state, 56_000.0);

    let (state, _) = current.apply(
        state,
        Event::RenewalFinished(
            renewal,
            current.cookie.clone(),
            RenewalOutcome::Succeeded(Rc::clone(&renewed.session)),
        ),
        57_000.0,
    );
    let (state, effects) = current.apply(state, read_snapshot.read_ok(check), 59_000.0);
    assert_eq!(state.view().session.as_deref(), Some(renewed.session.as_ref()));
    assert_eq!(armed_timer(&effects, Timer::Check).1, 57_000);

    let (state, effects) = current.apply(state, Event::Input { on_logout: false }, 59_200.0);
    let (timer, delay) = armed_timer(&effects, Timer::Renewal);
    assert_eq!(delay, 17_800);
    let (_, effects) = current.apply(state, Event::TimerElapsed(Timer::Renewal, timer), 77_000.0);
    assert!(matches!(effects.as_slice(), [Effect::Renew(_, session)] if
        Rc::ptr_eq(session, &renewed.session)));
}

#[test]
fn broadcast_read_keeps_same_generation_renewal_and_its_clock() {
    let current = SessionFixture::new("current-cookie", 60);
    let read_snapshot = current.projection_after(57, 3, 59_000.0);
    let renewed = current.projection_after(56, 60, 57_000.0);
    let (state, renewal) = current.start_renewal_at(current.authenticated(), 55_000.0);
    let (state, effects) = current.apply(state, Event::Broadcast, 56_000.0);
    let check = read_ticket(&effects);
    let (state, effects) = current.apply(
        state,
        Event::RenewalFinished(
            renewal,
            current.cookie.clone(),
            RenewalOutcome::Succeeded(Rc::clone(&renewed.session)),
        ),
        57_000.0,
    );
    assert!(state.view().session.is_none());
    assert!(!state.view().can_write);
    assert!(effects.is_empty());

    let (state, effects) = current.apply(state, read_snapshot.read_ok(check), 59_000.0);
    assert!(state.view().can_write);
    assert_eq!(state.view().session.as_deref(), Some(renewed.session.as_ref()));
    assert_eq!(armed_timer(&effects, Timer::Check).1, 57_000);
    let (_, effects) = current.apply(state, Event::Input { on_logout: false }, 59_200.0);
    assert_eq!(armed_timer(&effects, Timer::Renewal).1, 17_800);
}

fn broadcast_with_buffered_renewal() -> (SessionFixture, State, Ticket) {
    let current = SessionFixture::new("current-cookie", 60);
    let renewed = current.projection_after(1, 600, NOW);
    let (state, renewal) = current.start_renewal(current.authenticated());
    let (state, effects) = current.apply(state, Event::Broadcast, NOW);
    let check = read_ticket(&effects);
    let (state, _) = current.apply(
        state,
        Event::RenewalFinished(
            renewal,
            current.cookie.clone(),
            RenewalOutcome::Succeeded(renewed.session),
        ),
        NOW,
    );
    assert!(state.buffered_renewal.is_some());
    (current, state, check)
}

#[test]
fn broadcast_read_discards_renewal_for_replaced_cookie_or_identity() {
    let (current, state, check) = broadcast_with_buffered_renewal();
    let replacement = SessionFixture::new("new-cookie", 60);
    let (next, effects) = replacement.apply(state.clone(), current.read_ok(check), NOW);
    assert!(!next.view().can_write);
    assert!(next.buffered_renewal.is_none());
    let (next, _) = replacement.apply(next, replacement.read_ok(read_ticket(&effects)), NOW);
    assert_eq!(next.view().session.as_deref(), Some(replacement.session.as_ref()));

    let mut different_actor = current.session.as_ref().clone();
    different_actor.actor = "different-operator".to_owned();
    let same_cookie = SessionFixture { session: Rc::new(different_actor), cookie: current.cookie };
    let (next, _) = same_cookie.apply(state, same_cookie.read_ok(check), NOW);
    assert!(next.buffered_renewal.is_none());
    assert_eq!(next.view().session.as_deref(), Some(same_cookie.session.as_ref()));
}

#[test]
fn broadcast_read_discards_renewal_when_cookie_is_unavailable_or_anonymous() {
    let (current, state, check) = broadcast_with_buffered_renewal();
    let (next, _) = step(state.clone(), current.read_ok(check), Cookie::Unavailable, NOW);
    assert!(next.buffered_renewal.is_none());
    assert_eq!(next.view().session.as_deref(), Some(current.session.as_ref()));
    let (next, effects) = current.apply(
        state,
        Event::ReadFinished(check, current.cookie.clone(), ReadOutcome::Unauthenticated),
        NOW,
    );
    assert!(effects.is_empty());
    assert!(next.buffered_renewal.is_none());
    assert!(next.view().logged_out);
    assert!(!next.view().can_write);
}

#[test]
fn broadcast_read_failure_keeps_renewal_hidden_until_a_newer_read_confirms() {
    let (current, state, check) = broadcast_with_buffered_renewal();
    let (state, _) = current.apply(
        state,
        Event::ReadFinished(
            check,
            current.cookie.clone(),
            ReadOutcome::Failed("storage_unavailable".to_owned()),
        ),
        NOW,
    );
    assert!(state.buffered_renewal.is_some());
    assert!(state.view().session.is_none());
    assert!(!state.view().can_write);

    let (state, effects) = current.apply(state, Event::CheckRequested, NOW);
    let newer = current.projection_after(2, 700, NOW + 10.0);
    let (state, _) = current.apply(state, newer.read_ok(read_ticket(&effects)), NOW + 10.0);
    assert!(state.view().can_write);
    assert_eq!(state.view().session.as_deref(), Some(newer.session.as_ref()));
    assert!(state.buffered_renewal.is_none());
}

#[test]
fn renewal_after_logout_cannot_extend_the_next_session_review() {
    let current = SessionFixture::new("current-cookie", 60);
    let renewed = current.projection_after(1, 600, NOW);
    let (state, renewal) = current.start_renewal(current.authenticated());
    let (state, effects) = current.apply(state, Event::LogoutClicked, NOW);
    let logout = super::logout_ticket(&effects);
    let (state, _) = current.apply(state, Event::Broadcast, NOW);
    let (state, effects) = current.apply(
        state,
        Event::LogoutFinished(
            logout,
            current.cookie.clone(),
            super::LogoutOutcome::RateLimited("rate_limited".to_owned()),
        ),
        NOW,
    );
    let check = read_ticket(&effects);
    let (state, effects) = current.apply(
        state,
        Event::RenewalFinished(
            renewal,
            current.cookie.clone(),
            RenewalOutcome::Succeeded(renewed.session),
        ),
        NOW,
    );
    assert!(effects.is_empty());
    assert!(state.buffered_renewal.is_none());
    assert!(!state.view().can_write);
    let (state, _) = current.apply(state, current.read_ok(check), NOW);
    assert_eq!(state.view().session.as_deref(), Some(current.session.as_ref()));
}

#[test]
fn late_renewal_keeps_a_later_get_deadline_and_its_clock() {
    let current = SessionFixture::new("current-cookie", 60);
    let renewed = current.projection_after(10, 60, 13_000.0);
    let read_snapshot = current.projection_after(11, 65, 12_000.0);
    let (state, renewal) = current.start_renewal_at(current.authenticated(), 10_000.0);
    let (state, effects) = current.apply(state, Event::Visible, 11_000.0);
    let (state, _) = current.apply(state, read_snapshot.read_ok(read_ticket(&effects)), 12_000.0);
    let (state, effects) = current.apply(
        state,
        Event::RenewalFinished(
            renewal,
            current.cookie.clone(),
            RenewalOutcome::Succeeded(Rc::clone(&renewed.session)),
        ),
        13_000.0,
    );
    assert_eq!(state.view().session.as_deref(), Some(read_snapshot.session.as_ref()));
    assert_eq!(armed_timer(&effects, Timer::Check).1, 63_000);
}

#[test]
fn equal_deadlines_keep_the_later_server_observation_and_its_clock() {
    let current = SessionFixture::new("current-cookie", 60);
    let newer = current.projection_after(10, 50, 11_000.0);
    let older = current.projection_after(9, 51, 14_000.0);
    let (state, effects) = current.apply(current.authenticated(), Event::Visible, 10_000.0);
    let (state, _) = current.apply(state, newer.read_ok(read_ticket(&effects)), 11_000.0);
    assert_eq!(state.view().session.as_deref(), Some(newer.session.as_ref()));

    let (state, effects) = current.apply(state, Event::Visible, 12_000.0);
    let (state, effects) = current.apply(state, older.read_ok(read_ticket(&effects)), 14_000.0);
    assert_eq!(state.view().session.as_deref(), Some(newer.session.as_ref()));
    assert_eq!(armed_timer(&effects, Timer::Check).1, 46_000);
}
