//! 当前宿主的外部及续期失效证据、请求竞争与通知账本。

use std::rc::Rc;

use super::logout::{finish_logout, limited, read, start_logout, unknown};
use super::{NOW, SessionFixture, count_notices, read_ticket};
use crate::surface::auth::session::{
    Cookie, Effect, Event, ReadOutcome, RenewalOutcome, State, step,
};

#[derive(Clone, Copy)]
enum Review {
    Automatic,
    Explicit,
    Stay,
}

fn in_flight_review(a: &SessionFixture, state: State, reason: Review) -> (State, u64) {
    let (state, logout) = start_logout(a, state, Event::LogoutClicked);
    let (state, effects) = finish_logout(
        a,
        state,
        logout,
        &a.cookie,
        if matches!(reason, Review::Stay) { unknown() } else { limited() },
    );
    let (state, effects) = match reason {
        Review::Automatic => (state, effects),
        Review::Explicit => a.apply(state, Event::CheckRequested, NOW),
        Review::Stay => {
            let (state, _) = a.apply(state, a.read_ok(read_ticket(&effects)), NOW);
            a.apply(state, Event::StayAfterLogout, NOW)
        }
    };
    let ticket = read_ticket(&effects);
    let (state, _) = a.apply(state, Event::ReadSending(ticket), NOW);
    (state, ticket)
}

fn external(a: &SessionFixture, state: &State) -> Event {
    Event::ExternalUnauthenticated { epoch: state.view().epoch, session: a.session.clone() }
}

fn assert_held(state: &State, effects: &[Effect]) {
    assert!(!state.view().can_write);
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Logout(..) | Effect::Renew(..))));
}

#[test]
fn external_401_after_429_or_stay_invalidates_earlier_read_in_both_orders() {
    let a = SessionFixture::new("session-a", 900);
    for reason in [Review::Automatic, Review::Explicit, Review::Stay] {
        for old_first in [false, true] {
            for recover in [false, true] {
                let (state, old) = in_flight_review(&a, a.authenticated(), reason);
                let invalid = external(&a, &state);
                let state = if old_first {
                    let (state, _) = a.apply(state, a.read_ok(old), NOW);
                    assert!(state.view().can_write);
                    state
                } else {
                    state
                };
                let (state, effects) = a.apply(state, invalid, NOW);
                let fresh = read_ticket(&effects);
                assert_ne!(fresh, old);
                assert_eq!(count_notices(&effects), 0);
                assert_held(&state, &effects);
                assert!(state.view().key_visibility.hidden);
                let (state, effects) = a.apply(state, a.read_ok(old), NOW);
                assert!(effects.is_empty());
                assert_held(&state, &effects);
                let outcome = if recover {
                    ReadOutcome::Authenticated(a.session.clone())
                } else {
                    ReadOutcome::Unauthenticated
                };
                let (state, effects) = read(state, fresh, &a.cookie, outcome);
                assert_eq!(state.view().can_write, recover);
                assert_eq!(state.view().logged_out, !recover);
                assert_eq!(count_notices(&effects), usize::from(!recover));
            }
        }
    }
}

#[test]
fn two_external_401s_each_supersede_current_ticket_and_stale_evidence_is_inert() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for recover in [false, true] {
        let (state, effects) = a.apply(a.authenticated(), Event::CheckRequested, NOW);
        let old = read_ticket(&effects);
        let invalid = external(&a, &state);
        let (state, effects) = a.apply(state, invalid.clone(), NOW);
        let first = read_ticket(&effects);
        let (state, effects) = a.apply(state, invalid.clone(), NOW);
        let second = read_ticket(&effects);
        assert!(old < first && first < second);
        assert_eq!(count_notices(&effects), 0);
        let mut state = state;
        for event in [a.read_ok(old), a.read_ok(first)] {
            let (next, effects) = a.apply(state, event, NOW);
            assert!(effects.is_empty());
            assert_held(&next, &effects);
            state = next;
        }
        let result = if recover {
            ReadOutcome::Authenticated(a.session.clone())
        } else {
            ReadOutcome::Unauthenticated
        };
        let (state, effects) = read(state, second, &a.cookie, result);
        assert_eq!(state.view().can_write, recover);
        assert_eq!(count_notices(&effects), usize::from(!recover));
        if !recover {
            let (state, effects) = a.apply(state, invalid, NOW);
            assert!(state.view().logged_out && effects.is_empty());
        }
    }

    let old = a.authenticated();
    let invalid = external(&a, &old);
    let (state, effects) = a.apply(old, Event::Broadcast, NOW);
    let check = read_ticket(&effects);
    for owner in [&a, &b] {
        let (state, _) = read(
            state.clone(),
            check,
            &b.cookie,
            ReadOutcome::Failed("storage_unavailable".into()),
        );
        let (state, effects) = owner.apply(state, Event::CheckRequested, NOW);
        let (state, _) = owner.apply(state, owner.read_ok(read_ticket(&effects)), NOW);
        assert!(state.view().can_write);
        let before = state.view();
        let (state, effects) = step(state, invalid.clone(), Cookie::Missing, NOW);
        assert!(state.view() == before && effects.is_empty());
    }

    let current = a.authenticated();
    let before = current.view();
    let (next, effects) = a.apply(
        current,
        Event::ExternalUnauthenticated { epoch: before.epoch, session: b.session },
        NOW,
    );
    assert!(next.view() == before && effects.is_empty());
}

#[test]
fn external_401_and_renewal_receipts_resolve_without_old_get_reopening_writes() {
    let a = SessionFixture::new("session-a", 900);
    for reason in [Review::Explicit, Review::Stay] {
        for renewal_expires in [false, true] {
            for recover in [false, true] {
                for order in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
                    let (state, renewal) = a.start_renewal(a.authenticated());
                    let (state, _) = a.apply(state, Event::RenewalSending(renewal), NOW);
                    let (mut state, old) = in_flight_review(&a, state, reason);
                    let invalid = external(&a, &state);
                    let renewed = if renewal_expires {
                        RenewalOutcome::Unauthenticated
                    } else {
                        RenewalOutcome::Succeeded(a.projection_after(1, 900, NOW).session)
                    };
                    let events = [
                        a.read_ok(old),
                        invalid,
                        Event::RenewalFinished(renewal, a.cookie.clone(), renewed),
                    ];
                    let mut hits = 0;
                    let mut newest = None;
                    let mut expired = false;
                    let mut invalidated = false;
                    for index in order {
                        let (next, effects) = a.apply(state, events[index].clone(), NOW);
                        hits += count_notices(&effects);
                        if index == 1 {
                            if expired {
                                assert!(effects.is_empty());
                            } else {
                                let ticket = read_ticket(&effects);
                                assert_ne!(ticket, old);
                                newest = Some(ticket);
                                invalidated = true;
                            }
                        }
                        if index == 2 && renewal_expires {
                            assert_eq!(count_notices(&effects), 1);
                            newest = Some(read_ticket(&effects));
                            expired = true;
                        }
                        if invalidated || expired {
                            assert_held(&next, &effects);
                        }
                        state = next;
                    }
                    let result = if recover {
                        ReadOutcome::Authenticated(a.session.clone())
                    } else {
                        ReadOutcome::Unauthenticated
                    };
                    let (state, effects) = read(state, newest.unwrap(), &a.cookie, result);
                    assert_eq!(state.view().can_write, recover);
                    assert_eq!(
                        hits + count_notices(&effects),
                        usize::from(renewal_expires || !recover)
                    );
                }
            }
        }
    }
}

#[test]
fn renewal_401_from_retired_projection_wins_429_and_unknown_in_both_orders() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for reason in [Review::Automatic, Review::Stay] {
        for old_first in [false, true] {
            let (state, renewal) = a.start_renewal(a.authenticated());
            let (state, _) = a.apply(state, Event::RenewalSending(renewal), NOW);
            let (state, old) = in_flight_review(&a, state, reason);
            let state = if old_first { a.apply(state, a.read_ok(old), NOW).0 } else { state };
            let (next, effects) = a.apply(
                state.clone(),
                Event::RenewalFinished(renewal, a.cookie.clone(), RenewalOutcome::Unknown),
                NOW,
            );
            assert_eq!(count_notices(&effects), 0);
            assert!(next.view() == state.view());
            let (next, effects) = a.apply(
                state.clone(),
                Event::RenewalFinished(renewal, b.cookie.clone(), RenewalOutcome::Unauthenticated),
                NOW,
            );
            assert_eq!(count_notices(&effects), 0);
            assert!(next.view() == state.view());
            let receipt =
                Event::RenewalFinished(renewal, a.cookie.clone(), RenewalOutcome::Unauthenticated);
            let (state, effects) = a.apply(state, receipt.clone(), NOW);
            assert_eq!(count_notices(&effects), 1);
            assert_held(&state, &effects);
            let fresh = read_ticket(&effects);
            let (state, effects) = a.apply(state, a.read_ok(old), NOW);
            assert!(effects.is_empty());
            assert_held(&state, &effects);
            let (state, effects) = a.apply(state, receipt, NOW);
            assert!(effects.is_empty());
            let (state, _) = a.apply(state, a.read_ok(fresh), NOW);
            assert!(state.view().can_write);
        }
    }
}

#[test]
fn expiration_notice_survives_host_unmount_rate_limit_and_failed_read() {
    let a = SessionFixture::new("session-a", 900);
    for renewal_first in [false, true] {
        for peer in [false, true] {
            let (state, renewal) = a.start_renewal(a.authenticated());
            let (state, _) = a.apply(state, Event::RenewalSending(renewal), NOW);
            let (state, logout) = start_logout(&a, state, Event::LogoutClicked);
            let unauthorized =
                Event::RenewalFinished(renewal, a.cookie.clone(), RenewalOutcome::Unauthenticated);
            let (mut state, effects) = if renewal_first {
                let (state, effects) = a.apply(state, unauthorized.clone(), NOW);
                assert!(state.view().session.is_none() && effects.is_empty());
                finish_logout(&a, state, logout, &a.cookie, limited())
            } else {
                finish_logout(&a, state, logout, &a.cookie, limited())
            };
            let mut ticket = read_ticket(&effects);
            let mut notices = count_notices(&effects);
            if peer {
                let (next, effects) = a.apply(state, Event::Broadcast, NOW);
                ticket = read_ticket(&effects);
                notices += count_notices(&effects);
                state = next;
            }
            let (next, effects) =
                read(state, ticket, &a.cookie, ReadOutcome::Failed("storage_unavailable".into()));
            assert_eq!(count_notices(&effects), 0);
            let (state, effects) = a.apply(next, Event::CheckRequested, NOW);
            let (state, effects) =
                read(state, read_ticket(&effects), &a.cookie, ReadOutcome::Unauthenticated);
            assert!(state.view().logged_out);
            notices += count_notices(&effects);
            let (state, effects) = a.apply(state, unauthorized, NOW);
            assert!(state.view().logged_out && effects.is_empty());
            assert_eq!(notices, usize::from(!peer));
        }
    }
}

#[test]
fn settled_notification_releases_projection_and_deduplicates_until_new_identity() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    let state = a.authenticated();
    let invalid = external(&a, &state);
    let (state, effects) = a.apply(state, invalid, NOW);
    let (state, effects) =
        read(state, read_ticket(&effects), &a.cookie, ReadOutcome::Unauthenticated);
    assert_eq!(count_notices(&effects), 1);
    let old_session = Rc::downgrade(&a.session);
    let same_identity = SessionFixture::new("session-a", 900);
    drop(a);
    assert!(old_session.upgrade().is_none(), "账本结清后须释放旧投影");

    let a = same_identity;
    let (state, effects) = b.apply(state, Event::Visible, NOW);
    let (state, _) = read(
        state,
        read_ticket(&effects),
        &b.cookie,
        ReadOutcome::Failed("storage_unavailable".into()),
    );
    let (state, effects) = a.apply(state, Event::Visible, NOW);
    let (state, _) = a.apply(state, a.read_ok(read_ticket(&effects)), NOW);
    let (state, effects) = a.apply(state, Event::Visible, NOW);
    let old_a = read_ticket(&effects);
    let (state, effects) = read(state, old_a, &a.cookie, ReadOutcome::Unauthenticated);
    assert_eq!(count_notices(&effects), 0);

    let (state, effects) = b.apply(state, Event::Visible, NOW);
    let (state, _) = b.apply(state, b.read_ok(read_ticket(&effects)), NOW);
    let before = state.view();
    let (state, effects) =
        b.apply(state, Event::ReadFinished(old_a, a.cookie, ReadOutcome::Unauthenticated), NOW);
    assert!(state.view() == before && effects.is_empty());
    let (state, effects) = b.apply(state, Event::Visible, NOW);
    let (state, effects) =
        read(state, read_ticket(&effects), &b.cookie, ReadOutcome::Unauthenticated);
    assert!(state.view().logged_out);
    assert_eq!(count_notices(&effects), 1);
}

#[test]
fn write_review_body_shields_keys_and_disqualifies_notification_for_owner_only() {
    let a = SessionFixture::new("session-a", 900);
    let b = SessionFixture::new("session-b", 900);
    for cookie in [&Cookie::Missing, &Cookie::Unavailable] {
        let current = a.authenticated();
        let before = current.view();
        let (current, effects) = a.apply(
            current,
            Event::WriteReviewIdentity { epoch: before.epoch, session: a.session.clone() },
            NOW,
        );
        assert!(current.view() == before && effects.is_empty());
        let (current, effects) = step(
            current,
            Event::WriteReviewIdentity { epoch: before.epoch + 1, session: b.session.clone() },
            cookie.clone(),
            NOW,
        );
        assert!(current.view() == before && effects.is_empty());

        let (current, effects) = step(
            current,
            Event::WriteReviewIdentity { epoch: before.epoch, session: b.session.clone() },
            cookie.clone(),
            NOW,
        );
        let review = read_ticket(&effects);
        let view = current.view();
        assert_held(&current, &effects);
        assert!(
            view.key_visibility.hidden
                && view.key_visibility.revision > before.key_visibility.revision
        );
        assert_eq!(view.session, before.session);
        assert_eq!(view.epoch, before.epoch);
        let (current, _) =
            read(current, review, cookie, ReadOutcome::Failed("storage_unavailable".into()));
        assert!(current.view().key_visibility.hidden);
        let (current, effects) = step(current, Event::CheckRequested, Cookie::Missing, NOW);
        let (current, effects) =
            read(current, read_ticket(&effects), &Cookie::Missing, ReadOutcome::Unauthenticated);
        assert!(current.view().logged_out);
        assert_eq!(count_notices(&effects), 0);
    }
}
