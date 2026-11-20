//! Cookie 与正文身份的九处观测，以及匿名探测与本地登录竞争。

use super::{
    NOW, SessionFixture, armed_timer, count_notices, logged_out, logout_ticket, read_ticket,
};
use crate::surface::auth::session::{
    Cookie, Effect, Event, LogoutOutcome, ReadOutcome, RenewalOutcome, State, Timer, step,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Get,
    Activity,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Register,
    Send,
    Receive,
}

#[test]
fn nine_cookie_observations_share_identity_fence() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for request in [Request::Get, Request::Activity, Request::Delete] {
        for point in [Observation::Register, Observation::Send, Observation::Receive] {
            for cookie in [&a.cookie, &b.cookie, &Cookie::Missing] {
                // 成功 DELETE 正常清 Cookie 由退出账本测试覆盖；此处用未成功的回执。
                let case = format!("{request:?}/{point:?}/{}", cookie == &a.cookie);
                let (state, register) = match request {
                    Request::Get => (a.authenticated(), Event::Visible),
                    Request::Activity => {
                        let (state, effects) =
                            a.apply(a.authenticated(), Event::Input { on_logout: false }, NOW);
                        let (timer, delay) = armed_timer(&effects, Timer::Renewal);
                        assert_eq!(delay, 0);
                        (state, Event::TimerElapsed(Timer::Renewal, timer))
                    }
                    Request::Delete => (a.authenticated(), Event::LogoutClicked),
                };
                let before = state.view();
                let (state, observed, effects) = if point == Observation::Register {
                    let (state, effects) = step(state, register, cookie.clone(), NOW);
                    let sent = match request {
                        Request::Get => {
                            effects.iter().any(|effect| matches!(effect, Effect::Read(_)))
                        }
                        Request::Activity => {
                            effects.iter().any(|effect| matches!(effect, Effect::Renew(..)))
                        }
                        Request::Delete => {
                            effects.iter().any(|effect| matches!(effect, Effect::Logout(..)))
                        }
                    };
                    (state, sent, effects)
                } else {
                    let (state, effects) = a.apply(state, register, NOW);
                    let ticket = super::issued_ticket(&effects, |effect| match (request, effect) {
                        (Request::Get, Effect::Read(ticket))
                        | (Request::Activity, Effect::Renew(ticket, _))
                        | (Request::Delete, Effect::Logout(ticket, _)) => Some(*ticket),
                        _ => None,
                    });
                    let (send, receive) = match request {
                        Request::Get => (Event::ReadSending(ticket), a.read_ok(ticket)),
                        Request::Activity => (
                            Event::RenewalSending(ticket),
                            Event::RenewalFinished(
                                ticket,
                                a.cookie.clone(),
                                RenewalOutcome::Succeeded(a.projection_after(1, 900, NOW).session),
                            ),
                        ),
                        Request::Delete => (
                            Event::LogoutSending(ticket),
                            Event::LogoutFinished(
                                ticket,
                                a.cookie.clone(),
                                LogoutOutcome::RateLimited("rate_limited".into()),
                            ),
                        ),
                    };
                    if point == Observation::Send {
                        let (state, effects) = step(state, send, cookie.clone(), NOW);
                        let sent = match request {
                            Request::Get => state.has_read(ticket),
                            Request::Activity => state.has_renewal(ticket),
                            Request::Delete => state.has_logout(ticket),
                        };
                        (state, sent, effects)
                    } else {
                        let (state, _) = a.apply(state, send, NOW);
                        let (state, effects) = step(state, receive, cookie.clone(), NOW);
                        (state, false, effects)
                    }
                };
                let view = state.view();
                assert_eq!(view.epoch, before.epoch, "{case}");
                assert!(
                    view.session.as_deref().is_some_and(|owner| owner.same_identity(&a.session)),
                    "{case}"
                );
                assert_eq!(count_notices(&effects), 0, "{case}");
                if cookie == &a.cookie {
                    assert!(!view.key_visibility.hidden, "{case}");
                    assert_eq!(
                        view.key_visibility.revision, before.key_visibility.revision,
                        "{case}"
                    );
                    if point != Observation::Receive {
                        assert!(observed, "{case}");
                    }
                    assert_eq!(view.can_write, request != Request::Delete, "{case}");
                    if request == Request::Activity && point == Observation::Receive {
                        assert_eq!(
                            view.session.unwrap().expires_at,
                            a.projection_after(1, 900, NOW).session.expires_at
                        );
                    }
                } else {
                    assert!(!view.can_write && view.key_visibility.hidden, "{case}");
                    assert!(
                        view.key_visibility.revision > before.key_visibility.revision,
                        "{case}"
                    );
                    if point == Observation::Send && request == Request::Get {
                        // 登记 A 后实发时已见 B 或缺失，原票转成当前 Cookie 的唯一读取。
                        assert!(observed, "{case}");
                        assert!(
                            !effects.iter().any(|effect| matches!(effect, Effect::Read(_))),
                            "{case}"
                        );
                    } else if request != Request::Get || point == Observation::Receive {
                        assert!(!observed, "{case}");
                        read_ticket(&effects);
                    }
                }
            }
        }
    }
}

fn finish(
    state: State,
    ticket: u64,
    sent: &Cookie,
    observed: Cookie,
    outcome: ReadOutcome,
) -> (State, Vec<Effect>) {
    step(state, Event::ReadFinished(ticket, sent.clone(), outcome), observed, NOW)
}

#[test]
fn recheck_keeps_owner_and_key_revision_until_authoritative_confirmation() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    let before = a.authenticated().view();
    let (state, effects) = a.apply(a.authenticated(), Event::Visible, NOW);
    let ordinary = read_ticket(&effects);
    let (state, _) = finish(
        state,
        ordinary,
        &a.cookie,
        a.cookie.clone(),
        ReadOutcome::Failed("storage_unavailable".into()),
    );
    assert!(!state.view().can_write);
    assert!(state.view().key_visibility == before.key_visibility);

    for cookie in [&b.cookie, &Cookie::Missing] {
        let (state, effects) = a.apply(a.authenticated(), Event::Visible, NOW);
        let old = read_ticket(&effects);
        let (state, effects) = step(state, Event::Visible, cookie.clone(), NOW);
        let current = read_ticket(&effects);
        assert_ne!(old, current);
        let (state, effects) = finish(
            state,
            old,
            &a.cookie,
            cookie.clone(),
            ReadOutcome::Authenticated(a.session.clone()),
        );
        assert!(effects.is_empty());
        let (state, _) = finish(
            state,
            current,
            cookie,
            cookie.clone(),
            ReadOutcome::Failed("storage_unavailable".into()),
        );
        let hidden = state.view();
        assert!(hidden.key_visibility.hidden);
        assert!(!hidden.can_write);
        assert_eq!(hidden.epoch, before.epoch);
        assert_eq!(hidden.session, before.session);

        let (state, effects) = a.apply(state, Event::CheckRequested, NOW);
        let (state, _) = a.apply(state, a.read_ok(read_ticket(&effects)), NOW);
        assert!(state.view().can_write);
        assert!(!state.view().key_visibility.hidden);
        assert_eq!(state.view().key_visibility.revision, hidden.key_visibility.revision);
        assert_ne!(state.view().key_visibility.revision, before.key_visibility.revision);
    }
}

#[test]
fn renewal_receipt_after_503_observes_replacement_before_discarding_old_projection() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    let (state, renewal) = a.start_renewal(a.authenticated());
    let (state, _) = a.apply(state, Event::RenewalSending(renewal), NOW);
    let (state, effects) = a.apply(state, Event::Visible, NOW);
    let (state, _) = finish(
        state,
        read_ticket(&effects),
        &a.cookie,
        a.cookie.clone(),
        ReadOutcome::Failed("storage_unavailable".into()),
    );
    assert!(!state.view().key_visibility.hidden);

    let completed = Event::RenewalFinished(
        renewal,
        a.cookie.clone(),
        RenewalOutcome::Succeeded(a.projection_after(1, 900, NOW).session),
    );
    let (state, effects) = b.apply(state, completed, NOW);
    read_ticket(&effects);
    assert!(state.view().key_visibility.hidden && !state.view().can_write);
    assert_eq!(state.view().session.as_deref(), Some(a.session.as_ref()));

    let (state, effects) = step(state, Event::Visible, Cookie::Missing, NOW);
    let (state, effects) = finish(
        state,
        read_ticket(&effects),
        &Cookie::Missing,
        Cookie::Missing,
        ReadOutcome::Unauthenticated,
    );
    assert!(state.view().logged_out);
    assert_eq!(count_notices(&effects), 0);
}

#[test]
fn current_get_body_disqualifies_natural_expiration_even_when_cookie_cannot_confirm_it() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for (sent, received) in [
        (&a.cookie, &Cookie::Missing),
        (&Cookie::Missing, &Cookie::Missing),
        (&Cookie::Unavailable, &Cookie::Unavailable),
    ] {
        let (state, _) = step(a.authenticated(), Event::Visible, Cookie::Missing, NOW);
        let (state, effects) = step(state, Event::CheckRequested, sent.clone(), NOW);
        let (state, _) = finish(
            state,
            read_ticket(&effects),
            sent,
            received.clone(),
            ReadOutcome::Authenticated(b.session.clone()),
        );
        assert!(!state.view().can_write && state.view().key_visibility.hidden);
        let (state, effects) = step(state, Event::CheckRequested, Cookie::Missing, NOW);
        let (state, effects) = finish(
            state,
            read_ticket(&effects),
            &Cookie::Missing,
            Cookie::Missing,
            ReadOutcome::Unauthenticated,
        );
        assert!(state.view().logged_out);
        assert_eq!(count_notices(&effects), 0);
    }
}

#[test]
fn replaced_ticket_and_bad_body_cannot_disqualify_owned_expiration() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for bad_body in [false, true] {
        let (state, effects) = a.apply(a.authenticated(), Event::Visible, NOW);
        let old = read_ticket(&effects);
        let (state, effects) = step(state, Event::CheckRequested, Cookie::Missing, NOW);
        let active = read_ticket(&effects);
        let (state, effects) = finish(
            state,
            if bad_body { active } else { old },
            &Cookie::Missing,
            Cookie::Missing,
            if bad_body {
                ReadOutcome::Failed("invalid_response".into())
            } else {
                ReadOutcome::Authenticated(b.session.clone())
            },
        );
        let (state, active) = if bad_body {
            let (state, effects) = step(state, Event::CheckRequested, Cookie::Missing, NOW);
            (state, read_ticket(&effects))
        } else {
            assert!(effects.is_empty());
            (state, active)
        };
        let (state, effects) =
            finish(state, active, &Cookie::Missing, Cookie::Missing, ReadOutcome::Unauthenticated);
        assert!(state.view().logged_out);
        assert_eq!(count_notices(&effects), 1);
    }
}

#[test]
fn logout_click_during_identity_review_only_reads_even_after_cookie_returns() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    let (state, _) = b.apply(a.authenticated(), Event::Visible, NOW);
    let (state, effects) = a.apply(state, Event::LogoutClicked, NOW);
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Logout(..))));
    let (state, _) = a.apply(state, a.read_ok(read_ticket(&effects)), NOW);
    assert!(state.view().can_write);
    let (_, effects) = a.apply(state, Event::LogoutClicked, NOW);
    logout_ticket(&effects);
}

#[test]
fn anonymous_visibility_and_broadcast_share_a_probe_and_keep_login_on_negative_result() {
    let a = SessionFixture::new("session-a", 900);
    let mut state = logged_out();
    let login = state.start_login().unwrap();
    let (state, effects) =
        a.apply(state, Event::LoginFinished(login, Err("invalid_credentials".into())), NOW);
    let (state, _) = finish(
        state,
        read_ticket(&effects),
        &a.cookie,
        a.cookie.clone(),
        ReadOutcome::Unauthenticated,
    );
    for outcome in [
        ReadOutcome::Unauthenticated,
        ReadOutcome::Failed("storage_unavailable".into()),
        ReadOutcome::Authenticated(a.session.clone()),
    ] {
        let (mut hidden, effects) = a.apply(state.clone(), Event::Broadcast, NOW);
        let deferred = read_ticket(&effects);
        assert!(hidden.cancel_discovery_before_send(deferred, false));
        assert!(!hidden.has_read(deferred));
        let (hidden, effects) = a.apply(hidden, Event::Visible, NOW);
        let active = read_ticket(&effects);
        let (mut visible, effects) = a.apply(hidden, Event::Broadcast, NOW);
        assert!(effects.is_empty());
        assert!(!visible.cancel_discovery_before_send(active, true));
        let (visible, effects) = a.apply(visible, Event::Visible, NOW);
        assert!(effects.is_empty());
        let accepted = matches!(&outcome, ReadOutcome::Authenticated(_));
        let (next, _) = finish(visible, active, &a.cookie, a.cookie.clone(), outcome);
        assert_eq!(next.view().can_write, accepted);
        if accepted {
            assert!(!next.view().logged_out);
            assert!(next.view().epoch > state.view().epoch);
        } else {
            assert!(next.view() == state.view());
        }
    }
}

#[test]
fn passive_probe_and_broadcast_never_replace_local_login_result() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for probe_first in [false, true] {
        for old in [ReadOutcome::Authenticated(a.session.clone()), ReadOutcome::Unauthenticated] {
            let (mut state, effects) = a.apply(logged_out(), Event::Visible, NOW);
            let probe = read_ticket(&effects);
            let login = state.start_login().unwrap();
            let (mut pending, effects) = b.apply(state, Event::Visible, NOW);
            assert!(effects.is_empty());
            assert!(pending.start_login().is_none());
            if probe_first {
                let (next, effects) =
                    finish(pending, probe, &a.cookie, b.cookie.clone(), old.clone());
                assert!(effects.is_empty());
                assert!(next.view().login_pending && !next.view().can_write);
                pending = next;
            }
            let (pending, effects) =
                a.apply(pending, Event::LoginFinished(login, Err("request_timeout".into())), NOW);
            let login_get = read_ticket(&effects);
            let (pending, effects) = a.apply(pending, Event::Broadcast, NOW);
            assert!(effects.is_empty());
            assert!(
                pending.has_read(login_get)
                    && pending.view().logged_out
                    && pending.view().login_pending
            );
            let (pending, effects) = b.apply(pending, Event::Broadcast, NOW);
            let replacement = read_ticket(&effects);
            assert_ne!(replacement, login_get);
            assert!(pending.view().logged_out && pending.view().login_pending);

            let (pending, effects) =
                finish(pending, probe, &a.cookie, b.cookie.clone(), old.clone());
            assert!(effects.is_empty());
            let (pending, effects) = finish(pending, login_get, &a.cookie, b.cookie.clone(), old);
            assert!(effects.is_empty());
            assert!(!pending.view().can_write);
            let (pending, _) = b.apply(pending, b.read_ok(replacement), NOW);
            assert!(pending.view().can_write);
            assert_eq!(pending.view().session.as_deref(), Some(b.session.as_ref()));
        }
    }
}

#[test]
fn renewal_body_observes_replacement_only_in_its_own_epoch() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    let (state, renewal) = a.start_renewal(a.authenticated());
    let (state, _) = a.apply(state, Event::RenewalSending(renewal), NOW);
    let changed =
        Event::RenewalFinished(renewal, a.cookie.clone(), RenewalOutcome::Succeeded(b.session));
    let (pending, effects) = step(state.clone(), changed.clone(), Cookie::Unavailable, NOW);
    read_ticket(&effects);
    assert!(pending.view().key_visibility.hidden && !pending.view().can_write);
    assert_eq!(pending.view().session, state.view().session);
    let (pending, effects) = step(pending, Event::CheckRequested, Cookie::Missing, NOW);
    let (_, effects) = finish(
        pending,
        read_ticket(&effects),
        &Cookie::Missing,
        Cookie::Missing,
        ReadOutcome::Unauthenticated,
    );
    assert_eq!(count_notices(&effects), 0);

    let before = state.view().epoch;
    let (state, effects) = a.apply(state, Event::Broadcast, NOW);
    let (state, _) = a.apply(state, a.read_ok(read_ticket(&effects)), NOW);
    assert!(state.view().epoch > before && state.view().can_write);
    let confirmed = state.view();
    let (state, effects) = step(state, changed, Cookie::Unavailable, NOW);
    assert!(effects.is_empty() && state.view() == confirmed);
}

#[test]
fn login_body_is_evidence_for_expiration_ledger_but_does_not_open_writes() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for returned in [&a, &b] {
        let (state, renewal) = a.start_renewal(a.authenticated());
        let (state, _) = a.apply(state, Event::RenewalSending(renewal), NOW);
        let (state, logout) = super::logout::start_logout(&a, state, Event::LogoutClicked);
        let (state, effects) = a.apply(
            state,
            Event::RenewalFinished(renewal, a.cookie.clone(), RenewalOutcome::Unauthenticated),
            NOW,
        );
        assert_eq!(count_notices(&effects), 0);
        let (state, effects) =
            super::logout::finish_logout(&a, state, logout, &a.cookie, super::logout::limited());
        let (mut state, effects) = finish(
            state,
            read_ticket(&effects),
            &Cookie::Unavailable,
            Cookie::Unavailable,
            ReadOutcome::Unauthenticated,
        );
        assert!(state.view().logged_out && state.view().session.is_none());
        assert_eq!(count_notices(&effects), 0);
        let login = state.start_login().unwrap();
        let (state, effects) = step(
            state,
            Event::LoginFinished(login, Ok(returned.session.clone())),
            Cookie::Missing,
            NOW,
        );
        assert!(state.view().login_pending && !state.view().can_write);
        let (state, effects) = finish(
            state,
            read_ticket(&effects),
            &Cookie::Missing,
            Cookie::Missing,
            ReadOutcome::Unauthenticated,
        );
        assert!(state.view().logged_out && !state.view().can_write);
        assert_eq!(count_notices(&effects), usize::from(returned.cookie == a.cookie));
    }
}
