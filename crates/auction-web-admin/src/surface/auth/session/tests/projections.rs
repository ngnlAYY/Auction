use std::rc::Rc;

use super::{SessionFixture, armed_timer, read_ticket};
use crate::surface::auth::session::{Effect, Event, RenewalOutcome, Timer};

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
