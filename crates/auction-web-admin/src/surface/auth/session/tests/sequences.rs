use std::rc::Rc;
use std::time::Instant;

use super::{NOW, SessionFixture, read_ticket};
use crate::api::AdminSession;
use crate::surface::auth::session::{
    CheckReason, Cookie, Effect, Event, LogoutOutcome, Phase, ReadOutcome, RenewalOutcome, Request,
    State, Timer, step,
};

/// 默认套件穷举到长度 3（不到一秒）。长度 4 的完整穷举约 3800 万条序列、二十多秒，
/// 改动会话状态机时手工运行：
/// `cargo test --locked -p auction-web-admin --bin auction-web-admin -- --ignored exhaustive`。
#[test]
fn exhaustive_event_sequences_preserve_session_invariants() {
    run_exhaustive(3);
}

#[test]
#[ignore = "完整穷举耗时二十多秒，改动会话状态机时手工运行"]
fn exhaustive_event_sequences_up_to_length_four() {
    run_exhaustive(4);
}

fn run_exhaustive(max_length: u32) {
    let started = Instant::now();
    let cases = Cases {
        current: Projections::new("current-cookie"),
        previous: Projections::new("previous-cookie"),
    };
    let seeds = cases.initial_states();
    let count: u64 = seeds.iter().map(|seed| cases.visit(seed, 0, max_length)).sum();
    let event_count: u64 = EVENTS.iter().map(|event| if event.has_result() { 2 } else { 1 }).sum();
    assert_eq!(
        count,
        seeds.len() as u64 * (0..=max_length).map(|length| event_count.pow(length)).sum::<u64>()
    );
    eprintln!(
        "{} 种初始状态，{event_count} 种事件，{count} 条序列，耗时 {:?}",
        seeds.len(),
        started.elapsed()
    );
}

struct Cases {
    current: Projections,
    previous: Projections,
}

struct Projections {
    initial: SessionFixture,
    extended: SessionFixture,
}

impl Projections {
    fn new(token: &str) -> Self {
        let initial = SessionFixture::new(token, 60);
        let extended = initial.projection_after(1, 600, NOW);
        Self { initial, extended }
    }

    fn session(&self, projection: Projection) -> Rc<AdminSession> {
        match projection {
            Projection::Initial => Rc::clone(&self.initial.session),
            Projection::Extended => Rc::clone(&self.extended.session),
        }
    }
}

impl Cases {
    fn initial_states(&self) -> Vec<State> {
        let current = &self.current.initial;
        let starting = State::default();
        let (starting_check, effects) = current.apply(starting.clone(), Event::Start, NOW);
        let (logged_out, _) = current.apply(
            starting_check.clone(),
            Event::ReadFinished(
                read_ticket(&effects),
                current.cookie.clone(),
                ReadOutcome::Unauthenticated,
            ),
            NOW,
        );
        let authenticated = current.authenticated();
        let (routine, effects) = current.apply(authenticated.clone(), Event::Visible, NOW);
        let (unavailable, _) = current.apply(
            routine.clone(),
            Event::ReadFinished(
                read_ticket(&effects),
                current.cookie.clone(),
                ReadOutcome::Failed("storage_unavailable".to_owned()),
            ),
            NOW,
        );
        let (explicit, _) = current.apply(authenticated.clone(), Event::CheckRequested, NOW);
        let (logging_out, _) = current.apply(authenticated.clone(), Event::LogoutClicked, NOW);
        let (logout_with_broadcast, _) = current.apply(logging_out.clone(), Event::Broadcast, NOW);
        let (renewing, _) = current.start_renewal(authenticated.clone());
        let (renewing_with_input, _) =
            current.apply(renewing, Event::Input { on_logout: false }, NOW);
        vec![
            starting,
            starting_check,
            logged_out,
            authenticated,
            routine,
            unavailable,
            explicit,
            logging_out,
            logout_with_broadcast,
            renewing_with_input,
        ]
    }

    fn visit(&self, state: &State, depth: u32, max_length: u32) -> u64 {
        if depth == max_length {
            return 1;
        }
        let now = NOW + f64::from(depth) * 100.0;
        let mut count = 1;
        for &kind in EVENTS {
            for old in [false, true].into_iter().take(if kind.has_result() { 2 } else { 1 }) {
                let (event, observed) = self.event(state, kind, old);
                let (next, effects) = step(state.clone(), event.clone(), observed.clone(), now);
                verify_transition(state, &event, &observed, &next, &effects, now);
                count += self.visit(&next, depth + 1, max_length);
            }
        }
        count
    }

    fn event(&self, state: &State, kind: Kind, old: bool) -> (Event, Cookie) {
        let request = match kind {
            Kind::Read(_) => state.check.as_ref().map(|check| &check.request),
            Kind::Logout(_) => active_logout(state),
            Kind::Renewal(_) => state.renewal.as_ref(),
            _ => None,
        };
        let cookie = request
            .map(|request| &request.cookie)
            .or_else(|| state.phase.verified().map(|verified| &verified.cookie))
            .unwrap_or(&self.current.initial.cookie);
        let (fixture, replacement) = if *cookie == self.previous.initial.cookie {
            (&self.previous, &self.current)
        } else {
            (&self.current, &self.previous)
        };
        let observed = if old { &replacement.initial.cookie } else { cookie };
        let ticket = request.map_or(0, |request| request.ticket);
        let event = match kind {
            Kind::Start => Event::Start,
            Kind::Login => Event::LoginCompleted(fixture.session(Projection::Initial)),
            Kind::Explicit => Event::CheckRequested,
            Kind::Broadcast => Event::Broadcast,
            Kind::Visible => Event::Visible,
            Kind::Input(on_logout) => Event::Input { on_logout },
            Kind::Exit => Event::LogoutClicked,
            Kind::Timer(Timer::Check) => {
                Event::TimerElapsed(Timer::Check, state.check_timer.unwrap_or(0))
            }
            Kind::Timer(Timer::Renewal) => {
                Event::TimerElapsed(Timer::Renewal, state.renewal_timer.unwrap_or(0))
            }
            Kind::Read(result) => {
                let outcome = match result {
                    Read::Ok(projection) => ReadOutcome::Authenticated(fixture.session(projection)),
                    Read::Unauthorized => ReadOutcome::Unauthenticated,
                    Read::Failed => ReadOutcome::Failed("storage_unavailable".to_owned()),
                };
                Event::ReadFinished(ticket, cookie.clone(), outcome)
            }
            Kind::Logout(result) => {
                let outcome = match result {
                    Logout::Ok => LogoutOutcome::Succeeded,
                    Logout::Rejected => LogoutOutcome::Rejected("origin_forbidden".to_owned()),
                    Logout::Unknown => LogoutOutcome::Unknown("request_timeout".to_owned()),
                    Logout::Limited => LogoutOutcome::RateLimited("rate_limited".to_owned()),
                };
                Event::LogoutFinished(ticket, cookie.clone(), outcome)
            }
            Kind::Renewal(result) => {
                let outcome = match result {
                    Renewal::Ok(projection) => {
                        RenewalOutcome::Succeeded(fixture.session(projection))
                    }
                    Renewal::Unauthorized => RenewalOutcome::Unauthenticated,
                    Renewal::Limited(delay) => RenewalOutcome::RateLimited(delay),
                    Renewal::Csrf => RenewalOutcome::CsrfRejected,
                    Renewal::Unknown => RenewalOutcome::Unknown,
                    Renewal::Rejected => RenewalOutcome::Rejected,
                };
                Event::RenewalFinished(ticket, cookie.clone(), outcome)
            }
        };
        (event, observed.clone())
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Start,
    Login,
    Explicit,
    Broadcast,
    Visible,
    Input(bool),
    Exit,
    Timer(Timer),
    Read(Read),
    Logout(Logout),
    Renewal(Renewal),
}

impl Kind {
    fn has_result(self) -> bool {
        matches!(self, Self::Read(_) | Self::Logout(_) | Self::Renewal(_))
    }
}

#[derive(Clone, Copy)]
enum Projection {
    Initial,
    Extended,
}

#[derive(Clone, Copy)]
enum Read {
    Ok(Projection),
    Unauthorized,
    Failed,
}

#[derive(Clone, Copy)]
enum Logout {
    Ok,
    Rejected,
    Unknown,
    Limited,
}

#[derive(Clone, Copy)]
enum Renewal {
    Ok(Projection),
    Unauthorized,
    Limited(Option<u64>),
    Csrf,
    Unknown,
    Rejected,
}

const EVENTS: &[Kind] = &[
    Kind::Start,
    Kind::Login,
    Kind::Explicit,
    Kind::Broadcast,
    Kind::Visible,
    Kind::Input(false),
    Kind::Input(true),
    Kind::Exit,
    Kind::Timer(Timer::Check),
    Kind::Timer(Timer::Renewal),
    Kind::Read(Read::Ok(Projection::Initial)),
    Kind::Read(Read::Ok(Projection::Extended)),
    Kind::Read(Read::Unauthorized),
    Kind::Read(Read::Failed),
    Kind::Logout(Logout::Ok),
    Kind::Logout(Logout::Rejected),
    Kind::Logout(Logout::Unknown),
    Kind::Logout(Logout::Limited),
    Kind::Renewal(Renewal::Ok(Projection::Initial)),
    Kind::Renewal(Renewal::Ok(Projection::Extended)),
    Kind::Renewal(Renewal::Unauthorized),
    Kind::Renewal(Renewal::Limited(Some(10_000))),
    Kind::Renewal(Renewal::Limited(Some(120_000))),
    Kind::Renewal(Renewal::Limited(None)),
    Kind::Renewal(Renewal::Csrf),
    Kind::Renewal(Renewal::Unknown),
    Kind::Renewal(Renewal::Rejected),
];

fn active_logout(state: &State) -> Option<&Request> {
    match &state.phase {
        Phase::LoggingOut(_, logout) => Some(&logout.request),
        _ => None,
    }
}

#[derive(Default)]
struct Effects {
    reads: u8,
    writes: u8,
    notices: u8,
    renewal_delay: Option<u32>,
}

fn verify_transition(
    previous: &State,
    event: &Event,
    observed: &Cookie,
    next: &State,
    effects: &[Effect],
    now: f64,
) {
    let mut count = Effects::default();
    for effect in effects {
        match effect {
            Effect::Read(_) => count.reads += 1,
            Effect::Logout(..) | Effect::Renew(..) => count.writes += 1,
            Effect::BroadcastEnded => count.notices += 1,
            Effect::ArmTimer(Timer::Renewal, _, delay) => {
                assert!(count.renewal_delay.replace(*delay).is_none());
            }
            _ => {}
        }
    }
    let view = next.view();
    assert_eq!(view.can_write, matches!(next.phase, Phase::Authenticated(_)));
    assert_eq!(view.session.is_some(), next.phase.verified().is_some());
    assert!(count.reads <= 1 && count.writes <= 1 && count.notices <= 1);
    assert!(next.renewal.is_none() || next.renewal_timer.is_none());
    if let Some(check) = &next.check {
        assert!(
            matches!(next.phase, Phase::Checking(_))
                || (check.reason == CheckReason::Routine && view.can_write)
        );
    }
    if view.logging_out {
        assert!(next.check.is_none());
        assert!(next.check_timer.is_none());
        assert!(next.renewal_timer.is_none());
    }
    if matches!(event, Event::Input { on_logout: true }) {
        assert_eq!(next.activity.latest_input, previous.activity.latest_input);
        assert!(effects.is_empty());
    }
    if matches!(event, Event::Broadcast) {
        assert!(!view.can_write);
        assert!(view.session.is_none());
        assert_eq!(count.notices, 0);
    }

    if let Phase::LoggingOut(_, logout) = &previous.phase
        && !matches!(event, Event::LogoutFinished(ticket, ..) if *ticket == logout.request.ticket)
    {
        assert!(view.logging_out && !view.can_write);
        assert!(active_logout(next).is_some_and(|request| request.ticket == logout.request.ticket));
        assert_eq!((count.reads, count.writes, count.notices), (0, 0, 0));
        if matches!(event, Event::Broadcast) {
            assert!(matches!(&next.phase, Phase::LoggingOut(None, next_logout)
                if next_logout.review_after));
        }
    }

    match event {
        Event::ReadFinished(ticket, sent, outcome)
            if previous.check.as_ref().is_some_and(|check| check.request.ticket == *ticket) =>
        {
            if sent != observed {
                verify_replaced_generation(next, observed, &count, false);
            } else {
                match outcome {
                    ReadOutcome::Authenticated(session) => {
                        verify_projection(previous, next, session);
                    }
                    ReadOutcome::Unauthenticated => {
                        assert!(view.logged_out && view.session.is_none());
                        assert_eq!(count.notices, u8::from(previous.phase.verified().is_some()));
                    }
                    ReadOutcome::Failed(_) => {}
                }
            }
        }
        Event::LogoutFinished(ticket, sent, outcome)
            if active_logout(previous).is_some_and(|request| request.ticket == *ticket) =>
        {
            if sent != observed {
                verify_replaced_generation(next, observed, &count, false);
            } else if matches!(outcome, LogoutOutcome::Succeeded) {
                let deferred =
                    matches!(&previous.phase, Phase::LoggingOut(_, logout) if logout.review_after);
                assert!(!view.logging_out && !view.can_write && view.session.is_none());
                assert_eq!((count.notices, count.reads), (1, u8::from(deferred)));
            } else if matches!(outcome, LogoutOutcome::Unknown(_) | LogoutOutcome::RateLimited(_)) {
                assert!(!view.logging_out && !view.can_write);
                assert_eq!((count.notices, count.reads), (0, 1));
            }
        }
        Event::RenewalFinished(ticket, sent, outcome)
            if previous.renewal.as_ref().is_some_and(|request| request.ticket == *ticket) =>
        {
            if sent != observed {
                let already_reading = previous.check.as_ref().is_some_and(|check| {
                    check.reason == CheckReason::CookieChanged && &check.request.cookie == observed
                });
                verify_replaced_generation(next, observed, &count, already_reading);
            } else {
                match outcome {
                    RenewalOutcome::Unknown | RenewalOutcome::CsrfRejected => {
                        assert_eq!(count.writes, 0);
                        assert_eq!(count.reads, u8::from(!view.logging_out));
                        assert!(count.renewal_delay.is_none());
                        assert!(!view.can_write);
                        if view.logging_out {
                            assert!(matches!(&next.phase, Phase::LoggingOut(_, logout)
                                if logout.review_after));
                        }
                    }
                    RenewalOutcome::Unauthenticated => {
                        assert!(view.session.is_none() && !view.can_write);
                    }
                    RenewalOutcome::RateLimited(retry_after) => {
                        verify_rate_limit(previous, next, &count, *retry_after, now);
                    }
                    RenewalOutcome::Succeeded(session)
                        if !view.logging_out && previous.phase.verified().is_some() =>
                    {
                        verify_projection(previous, next, session);
                    }
                    RenewalOutcome::Succeeded(_) | RenewalOutcome::Rejected => {}
                }
            }
        }
        _ => {}
    }
}

fn verify_projection(previous: &State, next: &State, received: &Rc<AdminSession>) {
    let mut expected = received;
    if let Some(previous) = previous.phase.verified() {
        if !previous.session.same_identity(received) {
            return;
        }
        let known = &previous.session;
        if known.expires_at > received.expires_at
            || (known.expires_at == received.expires_at
                && known.server_time >= received.server_time)
        {
            expected = known;
        }
    }
    assert!(Rc::ptr_eq(&next.phase.verified().unwrap().session, expected));
}

fn verify_replaced_generation(
    next: &State,
    observed: &Cookie,
    count: &Effects,
    already_reading: bool,
) {
    let view = next.view();
    assert_eq!((count.writes, count.notices), (0, 0));
    assert!(!view.can_write);
    assert!(next.phase.verified().is_none_or(|verified| &verified.cookie == observed));
    if view.logging_out {
        assert!(matches!(&next.phase, Phase::LoggingOut(_, logout) if logout.review_after));
    } else {
        assert!(
            next.check.as_ref().is_some_and(|check| check.reason == CheckReason::CookieChanged
                && &check.request.cookie == observed)
        );
        assert_eq!(count.reads, u8::from(!already_reading));
    }
}

fn verify_rate_limit(
    previous: &State,
    next: &State,
    count: &Effects,
    retry_after: Option<u64>,
    now: f64,
) {
    assert_eq!((count.writes, count.notices, count.reads), (0, 0, 0));
    let delay = retry_after.map_or_else(
        || previous.activity.timing.map_or(5_000.0, |timing| timing.interval_millis),
        |millis| millis as f64,
    );
    let earliest = previous.activity.retry_not_before.max(now + delay);
    assert!(next.activity.retry_not_before >= earliest);
    let Phase::Authenticated(verified) = &next.phase else {
        assert_eq!(next.activity.latest_input, previous.activity.latest_input);
        assert!(count.renewal_delay.is_none());
        return;
    };
    let expiration = crate::surface::auth::activity::estimated_expiration_millis(&verified.session);
    let delay = (earliest - now).ceil() as u32;
    let retained_input =
        previous.activity.latest_input.filter(|_| now + f64::from(delay) < expiration);
    assert_eq!(count.renewal_delay, retained_input.map(|_| delay));
    assert_eq!(next.activity.latest_input, retained_input);
}
