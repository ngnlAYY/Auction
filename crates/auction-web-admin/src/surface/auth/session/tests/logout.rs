//! 退出意图、人工决定与结束通知的回归序列。

use super::{NOW, SessionFixture, count_notices, logout_ticket, read_ticket};
use crate::surface::auth::session::{
    Cookie, Effect, Event, LogoutOutcome, ReadOutcome, RenewalOutcome, State, step,
};

pub(super) fn start_logout(a: &SessionFixture, state: State, choice: Event) -> (State, u64) {
    let (state, effects) = a.apply(state, choice, NOW);
    let ticket = logout_ticket(&effects);
    let (state, _) = a.apply(state, Event::LogoutSending(ticket), NOW);
    assert!(state.has_logout(ticket));
    (state, ticket)
}

pub(super) fn finish_logout(
    a: &SessionFixture,
    state: State,
    ticket: u64,
    cookie: &Cookie,
    outcome: LogoutOutcome,
) -> (State, Vec<Effect>) {
    step(state, Event::LogoutFinished(ticket, a.cookie.clone(), outcome), cookie.clone(), NOW)
}

pub(super) fn unknown() -> LogoutOutcome {
    LogoutOutcome::Unknown("request_timeout".into())
}

pub(super) fn limited() -> LogoutOutcome {
    LogoutOutcome::RateLimited("rate_limited".into())
}

pub(super) fn read(
    state: State,
    ticket: u64,
    cookie: &Cookie,
    outcome: ReadOutcome,
) -> (State, Vec<Effect>) {
    let (state, _) = step(state, Event::ReadSending(ticket), cookie.clone(), NOW);
    step(state, Event::ReadFinished(ticket, cookie.clone(), outcome), cookie.clone(), NOW)
}

fn failed() -> ReadOutcome {
    ReadOutcome::Failed("storage_unavailable".into())
}

fn assert_no_writes(effects: &[Effect]) {
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Logout(..) | Effect::Renew(..))));
}

fn assert_exit_locked(state: &State, effects: &[Effect]) {
    let view = state.view();
    assert!(view.logout_unconfirmed && view.key_visibility.hidden && !view.can_write);
    assert_eq!(count_notices(effects), 0);
    assert_no_writes(effects);
}

#[test]
fn unknown_delete_then_get_failure_then_401_notifies_once() {
    let a = SessionFixture::new("session-a", 900);
    let (state, ticket) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
    let (state, effects) = finish_logout(&a, state, ticket, &Cookie::Missing, unknown());
    assert_exit_locked(&state, &effects);
    let (state, effects) = read(state, read_ticket(&effects), &Cookie::Missing, failed());
    assert_exit_locked(&state, &effects);

    let (state, effects) = step(state, Event::CheckRequested, Cookie::Missing, NOW);
    let active = read_ticket(&effects);
    let receipt = Event::ReadFinished(active, Cookie::Missing, ReadOutcome::Unauthenticated);
    let (state, effects) = step(state, receipt.clone(), Cookie::Missing, NOW);
    assert_eq!(count_notices(&effects), 1);
    assert!(state.view().logged_out);
    let (state, effects) = step(state, receipt, Cookie::Missing, NOW);
    assert!(effects.is_empty());
    let (state, effects) = step(state, Event::Visible, Cookie::Missing, NOW);
    let (_, effects) =
        read(state, read_ticket(&effects), &Cookie::Missing, ReadOutcome::Unauthenticated);
    assert_eq!(count_notices(&effects), 0);
}

#[test]
fn observed_replacement_and_late_delete_cannot_reclaim_get_notification() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for terminal_cookie in [&b.cookie, &Cookie::Missing] {
        for result in [limited(), unknown(), LogoutOutcome::Succeeded] {
            let (state, ticket) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
            let (state, effects) = b.apply(state, Event::Visible, NOW);
            assert!(effects.is_empty());
            assert!(state.view().key_visibility.hidden);
            let (state, effects) = finish_logout(&a, state, ticket, &b.cookie, result);
            assert_eq!(count_notices(&effects), 0);
            let first = read_ticket(&effects);
            let (state, effects) = step(state, Event::CheckRequested, terminal_cookie.clone(), NOW);
            let current = read_ticket(&effects);
            assert_ne!(first, current);
            let (state, effects) = step(
                state,
                Event::ReadFinished(first, b.cookie.clone(), ReadOutcome::Unauthenticated),
                terminal_cookie.clone(),
                NOW,
            );
            assert!(effects.is_empty() && !state.view().can_write);
            let (state, effects) =
                read(state, current, terminal_cookie, ReadOutcome::Unauthenticated);
            assert!(state.view().logged_out);
            assert_eq!(count_notices(&effects), 0);
        }
    }
}

#[test]
fn manual_success_can_notify_after_unavailable_read_revokes_inference() {
    let a = SessionFixture::new("session-a", 900);
    let (state, first) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
    let (state, effects) = finish_logout(&a, state, first, &a.cookie, unknown());
    let recheck = read_ticket(&effects);
    let (state, effects) = read(state, recheck, &Cookie::Unavailable, failed());
    assert_exit_locked(&state, &effects);

    let (without_retry, effects) = step(state.clone(), Event::CheckRequested, Cookie::Missing, NOW);
    let (without_retry, effects) =
        read(without_retry, read_ticket(&effects), &Cookie::Missing, ReadOutcome::Unauthenticated);
    assert_eq!(count_notices(&effects), 0);
    assert!(without_retry.view().logged_out);

    // 另一个分支从同一次失败复核继续，人工 DELETE 的完整回执有独立证据。
    let (state, retry) = start_logout(&a, state, Event::ContinueLogout);
    assert_ne!(first, retry);
    let (state, effects) = a.apply(state, Event::ContinueLogout, NOW);
    assert!(effects.is_empty());
    let (state, effects) =
        finish_logout(&a, state, retry, &Cookie::Missing, LogoutOutcome::Succeeded);
    assert_eq!(count_notices(&effects), 1);
    let (state, effects) =
        finish_logout(&a, state, retry, &Cookie::Missing, LogoutOutcome::Succeeded);
    assert!(state.view().logged_out && effects.is_empty());
}

#[test]
fn unsent_retry_does_not_consume_prior_delete_ticket() {
    let a = SessionFixture::new("session-a", 900);
    let (state, first) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
    let (state, effects) = finish_logout(&a, state, first, &a.cookie, unknown());
    let (state, _) = read(state, read_ticket(&effects), &a.cookie, failed());
    let (state, retry) = start_logout(&a, state, Event::ContinueLogout);
    let (state, effects) = a.apply(
        state,
        Event::LogoutNotSent(
            retry,
            a.cookie.clone(),
            LogoutOutcome::Rejected("not_sent:invalid_request".into()),
        ),
        NOW,
    );
    assert_exit_locked(&state, &effects);
    assert!(!state.view().logout_decision_busy);
    let (_, effects) = a.apply(state.clone(), Event::ContinueLogout, NOW);
    logout_ticket(&effects);
    // 收到明确未发出的重试后，先前已登记的实际派发仍能归属稳定缺失的 401。
    let (state, effects) = step(state, Event::CheckRequested, Cookie::Missing, NOW);
    let (state, effects) =
        read(state, read_ticket(&effects), &Cookie::Missing, ReadOutcome::Unauthenticated);
    assert!(state.view().logged_out);
    assert_eq!(count_notices(&effects), 1);
}

#[test]
fn verified_delete_clearing_cookie_notifies_even_after_observing_replacement() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for continuing in [false, true] {
        for terminal_cookie in [&Cookie::Missing, &b.cookie] {
            let state = if continuing {
                let (state, first) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
                let (state, effects) = finish_logout(&a, state, first, &a.cookie, unknown());
                read(state, read_ticket(&effects), &a.cookie, failed()).0
            } else {
                a.authenticated()
            };
            let (state, ticket) = start_logout(
                &a,
                state,
                if continuing { Event::ContinueLogout } else { Event::LogoutClicked },
            );
            let (state, effects) = b.apply(state, Event::Visible, NOW);
            assert!(effects.is_empty());
            let (state, effects) =
                finish_logout(&a, state, ticket, terminal_cookie, LogoutOutcome::Succeeded);
            let cleared = terminal_cookie == &Cookie::Missing;
            assert_eq!(count_notices(&effects), usize::from(cleared));
            assert_eq!(state.view().session.is_none(), cleared);
            let check = read_ticket(&effects);
            let (state, effects) =
                finish_logout(&a, state, ticket, terminal_cookie, LogoutOutcome::Succeeded);
            assert_eq!(count_notices(&effects), 0);
            let (state, effects) =
                read(state, check, terminal_cookie, ReadOutcome::Unauthenticated);
            assert!(state.view().logged_out);
            assert_eq!(count_notices(&effects), 0);
        }
    }
}

#[test]
fn peer_notice_consumes_local_exit_after_failed_or_replaced_review() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for seen_replacement in [false, true] {
        let (state, ticket) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
        let (state, effects) = finish_logout(&a, state, ticket, &Cookie::Missing, unknown());
        let first = read_ticket(&effects);
        let (state, effects) = step(state, Event::Broadcast, Cookie::Missing, NOW);
        assert_eq!(count_notices(&effects), 0);
        let check = read_ticket(&effects);
        assert_ne!(check, first);
        let (state, effects) = read(state, check, &Cookie::Missing, failed());
        assert_eq!(count_notices(&effects), 0);
        let state = if seen_replacement {
            let (state, effects) = b.apply(state, Event::Visible, NOW);
            read_ticket(&effects);
            state
        } else {
            state
        };
        let (state, effects) = step(state, Event::CheckRequested, Cookie::Missing, NOW);
        let (state, effects) =
            read(state, read_ticket(&effects), &Cookie::Missing, ReadOutcome::Unauthenticated);
        assert!(state.view().logged_out);
        assert_eq!(count_notices(&effects), 0);
    }

    let (state, ticket) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
    let (state, effects) =
        finish_logout(&a, state, ticket, &Cookie::Missing, LogoutOutcome::Succeeded);
    assert_eq!(count_notices(&effects), 1);
    let (state, effects) = step(state, Event::Broadcast, Cookie::Missing, NOW);
    let (_, effects) =
        read(state, read_ticket(&effects), &Cookie::Missing, ReadOutcome::Unauthenticated);
    assert_eq!(count_notices(&effects), 0);
}

#[test]
fn unknown_exit_stays_locked_through_automatic_reads_activity_and_retry_429() {
    let a = SessionFixture::new("session-a", 900);
    let (state, renewal) = a.start_renewal(a.authenticated());
    let (state, _) = a.apply(state, Event::RenewalSending(renewal), NOW);
    let (state, first) = start_logout(&a, state, Event::LogoutClicked);
    let (state, effects) = finish_logout(&a, state, first, &a.cookie, unknown());
    let automatic = read_ticket(&effects);
    assert_exit_locked(&state, &effects);
    assert!(state.view().logout_decision_busy);
    let (state, effects) = a.apply(state, Event::StayAfterLogout, NOW);
    assert!(effects.is_empty());
    let (state, effects) = a.apply(state, Event::ContinueLogout, NOW);
    assert!(effects.is_empty());
    assert!(state.view().logout_decision_busy);

    let (mut state, effects) = a.apply(state, a.read_ok(automatic), NOW);
    assert_exit_locked(&state, &effects);
    for event in [
        Event::Visible,
        Event::Input { on_logout: false },
        Event::TimerElapsed(crate::surface::auth::session::Timer::Renewal, renewal),
        Event::RenewalFinished(
            renewal,
            a.cookie.clone(),
            RenewalOutcome::Succeeded(a.projection_after(1, 900, NOW).session),
        ),
        Event::CheckRequested,
        Event::LogoutClicked,
    ] {
        let (next, effects) = a.apply(state, event, NOW);
        assert_exit_locked(&next, &effects);
        state = if let Some(Effect::Read(ticket)) =
            effects.iter().find(|effect| matches!(effect, Effect::Read(_)))
        {
            let (next, effects) = a.apply(next, a.read_ok(*ticket), NOW);
            assert_exit_locked(&next, &effects);
            next
        } else {
            next
        };
    }
    let (state, retry) = start_logout(&a, state, Event::ContinueLogout);
    let (state, effects) = finish_logout(&a, state, retry, &a.cookie, limited());
    assert_exit_locked(&state, &effects);
    assert!(!state.view().logout_decision_busy);
    assert_eq!(state.view().logout_error.as_deref(), Some("rate_limited"));
}

#[test]
fn reobserving_same_identity_keeps_exit_locked_without_deferring_an_extra_get() {
    let a = SessionFixture::new("session-a", 900);
    let (state, first) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
    let (state, effects) = finish_logout(&a, state, first, &Cookie::Missing, unknown());
    let old = read_ticket(&effects);
    let (state, effects) = a.apply(state, Event::Visible, NOW);
    let active = read_ticket(&effects);
    assert_ne!(old, active);
    let (state, effects) = a.apply(state, a.read_ok(active), NOW);
    assert_exit_locked(&state, &effects);
    let (state, retry) = start_logout(&a, state, Event::ContinueLogout);
    let (state, _) = a.apply(state, Event::Visible, NOW);
    let (state, effects) =
        finish_logout(&a, state, retry, &Cookie::Missing, LogoutOutcome::Succeeded);
    assert!(state.view().logged_out);
    assert_eq!(count_notices(&effects), 1);
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Read(_))));
}

#[test]
fn continuing_logout_checks_cookie_at_click_and_send() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for cookie in [&b.cookie, &Cookie::Missing] {
        let (state, first) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
        let (state, effects) = finish_logout(&a, state, first, &a.cookie, unknown());
        let (state, _) = read(state, read_ticket(&effects), &a.cookie, failed());
        let (clicked, effects) = step(state.clone(), Event::ContinueLogout, cookie.clone(), NOW);
        assert_exit_locked(&clicked, &effects);
        read_ticket(&effects);
        let (state, effects) = a.apply(state, Event::ContinueLogout, NOW);
        let retry = logout_ticket(&effects);
        let (state, effects) = step(state, Event::LogoutSending(retry), cookie.clone(), NOW);
        assert!(!state.has_logout(retry));
        assert_exit_locked(&state, &effects);
        let (state, effects) =
            read(state, read_ticket(&effects), cookie, ReadOutcome::Unauthenticated);
        assert!(state.view().logged_out);
        assert_eq!(count_notices(&effects), usize::from(cookie == &Cookie::Missing));
    }
}

#[test]
fn stay_requires_its_own_current_same_generation_result() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    let (state, first) = start_logout(&a, a.authenticated(), Event::LogoutClicked);
    let (state, effects) = finish_logout(&a, state, first, &a.cookie, unknown());
    let old = read_ticket(&effects);
    let (state, _) = read(state, old, &a.cookie, failed());
    let (state, effects) = a.apply(state, Event::StayAfterLogout, NOW);
    let stay = read_ticket(&effects);
    let (changed, effects) = b.apply(state.clone(), Event::Visible, NOW);
    let changed = read(
        changed,
        read_ticket(&effects),
        &b.cookie,
        ReadOutcome::Failed("storage_unavailable".into()),
    )
    .0;
    let (changed, effects) = b.apply(changed, Event::StayAfterLogout, NOW);
    let changed_stay = read_ticket(&effects);
    let (changed, effects) = a.apply(changed, Event::ReadSending(changed_stay), NOW);
    assert_no_writes(&effects);
    let (changed, effects) = a.apply(changed, a.read_ok(changed_stay), NOW);
    assert_exit_locked(&changed, &effects);

    let (state, effects) = a.apply(state, Event::StayAfterLogout, NOW);
    assert!(effects.is_empty());
    let (state, effects) = a.apply(state, a.read_ok(old), NOW);
    assert_exit_locked(&state, &effects);
    assert!(state.has_read(stay));
    let (failed_state, effects) = read(state.clone(), stay, &a.cookie, failed());
    assert_exit_locked(&failed_state, &effects);
    let (failed_state, effects) = a.apply(failed_state, Event::StayAfterLogout, NOW);
    let (confirmed, _) = a.apply(failed_state, a.read_ok(read_ticket(&effects)), NOW);
    assert!(confirmed.view().can_write && !confirmed.view().logout_unconfirmed);

    let (expired, effects) = read(state.clone(), stay, &a.cookie, ReadOutcome::Unauthenticated);
    assert!(expired.view().logged_out);
    assert_eq!(count_notices(&effects), 1);
    let (state, effects) = b.apply(state, Event::ReadSending(stay), NOW);
    assert_no_writes(&effects);
    let (state, effects) = b.apply(state, b.read_ok(stay), NOW);
    assert!(state.view().logged_out && !state.view().can_write);
    assert_eq!(count_notices(&effects), 0);
}
