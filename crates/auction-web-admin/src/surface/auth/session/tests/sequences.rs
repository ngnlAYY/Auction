//! 在可达起点上逐步枚举会话事件，核对写许可、展示与副作用。

use std::rc::Rc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

use super::{NOW, SessionFixture, login_after_old_session_expires, read_ticket};
use crate::api::AdminSession;
use crate::surface::auth::session::{
    CheckReason, Cookie, Effect, Event, LogoutOutcome, Phase, ReadOutcome, RenewalOutcome, Request,
    State, Timer, View, step,
};

// 62 种事件。2 个退出流程起点穷举长度 0–3，其余 13 个起点穷举长度 0–2，
// 合计 535,261 条序列，本机调试构建单线程约 1.26 秒。长度 4 的竞争在下方独立穷举。
#[test]
fn exhaustive_event_sequences_preserve_session_invariants() {
    with_timeout(|| {
        let cases = Cases::new();
        let (competing, other) = cases.initial_states();
        let count = run_exhaustive(&cases, &competing, EVENTS, 3)
            + run_exhaustive(&cases, &other, EVENTS, 2);
        assert_eq!(count, 535_261);
    });
}

// 聚焦回执争用的 2 个可达起点 × 19 种事件，长度 0–4 合计 275,122 条序列，
// 本机调试构建单线程约 0.54 秒。
#[test]
fn exhaustive_event_sequences_up_to_length_four() {
    with_timeout(|| {
        let cases = Cases::new();
        let a = &cases.current.initial;
        let (renewing, _) = a.start_renewal(a.authenticated());
        let (logging_out, ticket) = super::logout::start_logout(a, renewing, Event::LogoutClicked);
        let (checking, _) = super::logout::finish_logout(
            a,
            logging_out.clone(),
            ticket,
            &a.cookie,
            super::logout::limited(),
        );
        run_exhaustive(&cases, &[logging_out, checking], COMPETING_RECEIPTS, 4);
    });
}

fn with_timeout(test: fn()) {
    let (finished, result) = channel();
    let worker = thread::spawn(move || {
        test();
        let _sent = finished.send(());
    });
    assert_ne!(
        result.recv_timeout(Duration::from_secs(20)),
        Err(RecvTimeoutError::Timeout),
        "穷举超过 20 秒"
    );
    if let Err(error) = worker.join() {
        std::panic::resume_unwind(error);
    }
}

fn run_exhaustive(cases: &Cases, seeds: &[State], alphabet: &[Kind], max_length: u32) -> u64 {
    let started = Instant::now();
    let count: u64 = seeds
        .iter()
        .map(|seed| cases.visit(seed, alphabet, 0, max_length, Observations::new(seed)))
        .sum();
    let event_count: u64 =
        alphabet.iter().map(|event| if event.has_result() { 2 } else { 1 }).sum();
    assert_eq!(
        count,
        seeds.len() as u64 * (0..=max_length).map(|length| event_count.pow(length)).sum::<u64>()
    );
    eprintln!(
        "{} 种初始状态，{event_count} 种事件，{count} 条序列，耗时 {:?}",
        seeds.len(),
        started.elapsed()
    );
    count
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
    fn new() -> Self {
        Self {
            current: Projections::new("current-cookie"),
            previous: Projections::new("previous-cookie"),
        }
    }

    fn initial_states(&self) -> ([State; 2], [State; 13]) {
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
        let (exit, ticket) =
            super::logout::start_logout(current, authenticated.clone(), Event::LogoutClicked);
        let (unknown_exit, _) = super::logout::finish_logout(
            current,
            exit,
            ticket,
            &current.cookie,
            super::logout::unknown(),
        );
        let (renewing, _) = current.start_renewal(authenticated.clone());
        let (logout_with_renewal, _) = current.apply(renewing.clone(), Event::LogoutClicked, NOW);
        let (renewing_with_input, _) =
            current.apply(renewing, Event::Input { on_logout: false }, NOW);
        let mut logging_in = logged_out.clone();
        logging_in.start_login().unwrap();
        let (login_with_broadcast, _) =
            step(logging_in.clone(), Event::Broadcast, Cookie::Missing, NOW);
        let (login_with_old_renewal, _, _) =
            login_after_old_session_expires(&self.previous.initial);
        (
            [logout_with_renewal, unknown_exit],
            [
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
                logging_in,
                login_with_broadcast,
                login_with_old_renewal,
            ],
        )
    }

    fn visit(
        &self,
        state: &State,
        alphabet: &[Kind],
        depth: u32,
        max_length: u32,
        history: Observations,
    ) -> u64 {
        if depth == max_length {
            return 1;
        }
        let now = NOW + f64::from(depth) * 100.0;
        let mut count = 1;
        for &kind in alphabet {
            for old in [false, true].into_iter().take(if kind.has_result() { 2 } else { 1 }) {
                let (event, observed) = self.event(state, kind, old);
                let (next, effects) = step(state.clone(), event.clone(), observed.clone(), now);
                let history = history.check(state, &event, &observed, &next, &effects);
                count += self.visit(&next, alphabet, depth + 1, max_length, history);
            }
        }
        count
    }

    fn event(&self, state: &State, kind: Kind, old: bool) -> (Event, Cookie) {
        let request = match kind {
            Kind::Read(_)
            | Kind::ReadSending
            | Kind::Unreadable(Unreadable::ReadSending | Unreadable::ReadUnauthorized) => {
                state.check.as_ref().map(|check| &check.request)
            }
            Kind::Logout(_)
            | Kind::LogoutNotSent
            | Kind::LogoutSending
            | Kind::Unreadable(Unreadable::LogoutSending) => active_logout(state),
            Kind::Renewal(_)
            | Kind::RenewalSending
            | Kind::Unreadable(Unreadable::RenewalSending) => {
                state.renewal.as_ref().map(|renewal| &renewal.request)
            }
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
        let observed = match kind {
            Kind::BroadcastAfterCookieClear | Kind::Logout(Logout::Cleared) => &Cookie::Missing,
            Kind::Unreadable(_) => &Cookie::Unavailable,
            Kind::LoginFinished if old => &Cookie::Missing,
            _ if old => &replacement.initial.cookie,
            _ => cookie,
        };
        let ticket = request.map_or(0, |request| request.ticket);
        let event = match kind {
            Kind::Start => Event::Start,
            Kind::LoginFinished => Event::LoginFinished(
                state.login.unwrap_or(0),
                if old {
                    Err("request_timeout".to_owned())
                } else {
                    Ok(fixture.session(Projection::Initial))
                },
            ),
            Kind::Explicit => Event::CheckRequested,
            Kind::Broadcast | Kind::BroadcastAfterCookieClear => Event::Broadcast,
            Kind::Visible | Kind::Unreadable(Unreadable::Visible) => Event::Visible,
            Kind::Input(on_logout) => Event::Input { on_logout },
            Kind::Exit => Event::LogoutClicked,
            Kind::Continue => Event::ContinueLogout,
            Kind::Stay => Event::StayAfterLogout,
            Kind::ReadSending | Kind::Unreadable(Unreadable::ReadSending) => {
                Event::ReadSending(ticket)
            }
            Kind::LogoutSending | Kind::Unreadable(Unreadable::LogoutSending) => {
                Event::LogoutSending(ticket)
            }
            Kind::RenewalSending | Kind::Unreadable(Unreadable::RenewalSending) => {
                Event::RenewalSending(ticket)
            }
            Kind::OldRead => Event::ReadFinished(
                0,
                cookie.clone(),
                ReadOutcome::Authenticated(fixture.session(Projection::Initial)),
            ),
            Kind::External401 => Event::ExternalUnauthenticated {
                epoch: state.view_epoch + u64::from(old),
                session: state.phase.verified().map_or_else(
                    || fixture.session(Projection::Initial),
                    |current| current.session.clone(),
                ),
            },
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
            Kind::Unreadable(Unreadable::ReadUnauthorized) => {
                Event::ReadFinished(ticket, cookie.clone(), ReadOutcome::Unauthenticated)
            }
            Kind::LogoutNotSent => Event::LogoutNotSent(
                ticket,
                cookie.clone(),
                LogoutOutcome::Rejected("invalid_request".to_owned()),
            ),
            Kind::Logout(result) => {
                let outcome = match result {
                    Logout::Ok | Logout::Cleared => LogoutOutcome::Succeeded,
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
    LoginFinished,
    Explicit,
    Broadcast,
    BroadcastAfterCookieClear,
    Visible,
    Input(bool),
    Exit,
    Continue,
    Stay,
    ReadSending,
    LogoutSending,
    RenewalSending,
    OldRead,
    External401,
    Unreadable(Unreadable),
    Timer(Timer),
    Read(Read),
    Logout(Logout),
    LogoutNotSent,
    Renewal(Renewal),
}

impl Kind {
    fn has_result(self) -> bool {
        matches!(
            self,
            Self::LoginFinished
                | Self::External401
                | Self::Read(_)
                | Self::Logout(Logout::Ok | Logout::Rejected | Logout::Unknown | Logout::Limited)
                | Self::LogoutNotSent
                | Self::Renewal(_)
        )
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
    Cleared,
    Rejected,
    Unknown,
    Limited,
}

#[derive(Clone, Copy)]
enum Unreadable {
    Visible,
    ReadSending,
    LogoutSending,
    RenewalSending,
    ReadUnauthorized,
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
    Kind::LoginFinished,
    Kind::Explicit,
    Kind::Broadcast,
    Kind::BroadcastAfterCookieClear,
    Kind::Visible,
    Kind::Input(false),
    Kind::Input(true),
    Kind::Exit,
    Kind::Continue,
    Kind::Stay,
    Kind::ReadSending,
    Kind::LogoutSending,
    Kind::RenewalSending,
    Kind::Unreadable(Unreadable::Visible),
    Kind::Unreadable(Unreadable::ReadSending),
    Kind::Unreadable(Unreadable::LogoutSending),
    Kind::Unreadable(Unreadable::RenewalSending),
    Kind::Unreadable(Unreadable::ReadUnauthorized),
    Kind::OldRead,
    Kind::External401,
    Kind::Timer(Timer::Check),
    Kind::Timer(Timer::Renewal),
    Kind::Read(Read::Ok(Projection::Initial)),
    Kind::Read(Read::Ok(Projection::Extended)),
    Kind::Read(Read::Unauthorized),
    Kind::Read(Read::Failed),
    Kind::Logout(Logout::Ok),
    Kind::Logout(Logout::Cleared),
    Kind::Logout(Logout::Rejected),
    Kind::LogoutNotSent,
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

const COMPETING_RECEIPTS: &[Kind] = &[
    Kind::Broadcast,
    Kind::Visible,
    Kind::Continue,
    Kind::Stay,
    Kind::ReadSending,
    Kind::LogoutSending,
    Kind::RenewalSending,
    Kind::OldRead,
    Kind::External401,
    Kind::Read(Read::Ok(Projection::Initial)),
    Kind::Read(Read::Unauthorized),
    Kind::Logout(Logout::Cleared),
    Kind::Logout(Logout::Limited),
    Kind::Renewal(Renewal::Unauthorized),
];

#[derive(Clone, Copy)]
struct Observations {
    last_writable_generation: Option<&'static str>,
    notified: bool,
}

impl Observations {
    fn generation(view: &View) -> Option<&'static str> {
        if !view.can_write {
            return None;
        }
        Some(match view.session.as_ref().unwrap().csrf_token.as_str() {
            "current-cookie" => "current-cookie",
            "previous-cookie" => "previous-cookie",
            token => panic!("未知测试代际 {token}"),
        })
    }

    fn new(seed: &State) -> Self {
        Self { last_writable_generation: Self::generation(&seed.view()), notified: false }
    }

    fn check(
        mut self,
        before: &State,
        event: &Event,
        observed: &Cookie,
        after: &State,
        effects: &[Effect],
    ) -> Self {
        let notices = super::count_notices(effects);
        let reads = effects.iter().filter(|effect| matches!(effect, Effect::Read(_))).count();
        let logout = effects.iter().any(|effect| matches!(effect, Effect::Logout(..)));
        let renew = effects.iter().any(|effect| matches!(effect, Effect::Renew(..)));
        let previous = before.view();
        let view = after.view();
        assert!(reads <= 1 && notices <= 1);
        if notices == 1 {
            assert!(matches!(
                event,
                Event::ReadFinished(_, _, ReadOutcome::Unauthenticated)
                    | Event::RenewalFinished(_, _, RenewalOutcome::Unauthenticated)
                    | Event::LogoutFinished(_, _, LogoutOutcome::Succeeded)
            ));
            assert!(!self.notified, "同一标签页同代重复通知");
            self.notified = true;
        }
        if let Some(generation) = Self::generation(&view)
            && self.last_writable_generation != Some(generation)
        {
            self.last_writable_generation = Some(generation);
            self.notified = false;
        }

        if view.can_write {
            assert!(matches!(after.phase, Phase::Authenticated(_)));
            assert!(view.session.is_some() && !view.key_visibility.hidden);
            assert!(!view.logout_unconfirmed && !view.logging_out && !view.logged_out);
        }
        if view.logout_unconfirmed {
            assert!(view.key_visibility.hidden && !view.can_write);
            assert!(!renew);
        }
        if previous.logout_unconfirmed && view.can_write {
            assert!(matches!(
                event,
                Event::ReadFinished(ticket, _, ReadOutcome::Authenticated(_))
                    if before.check.as_ref().is_some_and(|check|
                        check.request.ticket == *ticket && check.reason == CheckReason::StayAfterLogout)
            ));
        }
        assert!(view.key_visibility.revision >= previous.key_visibility.revision);
        if view.key_visibility.hidden && !previous.key_visibility.hidden {
            assert!(view.key_visibility.revision > previous.key_visibility.revision);
        }
        if previous.login_pending {
            assert!(!logout && !renew && notices == 0);
        }
        if logout {
            assert!(matches!(event, Event::LogoutClicked | Event::ContinueLogout));
            assert!(view.logging_out && !view.can_write);
        }

        // 实际看见另一代或缺失后，可以保留原宿主，但该投影不可开写或展示 Key。
        if let Some(owner) = &view.session
            && !matches!(observed, Cookie::Unavailable)
            && Cookie::from_session(owner) != *observed
            && !matches!(event, Event::ExternalUnauthenticated { epoch, .. } if *epoch != previous.epoch)
        {
            assert!(!view.can_write && view.key_visibility.hidden);
        }

        if let Event::ExternalUnauthenticated { epoch, session } = event {
            let owned = *epoch == previous.epoch
                && previous.session.as_ref().is_some_and(|current| current.same_identity(session));
            if owned {
                assert!(!view.can_write && view.key_visibility.hidden && notices == 0);
                assert_eq!(reads, usize::from(!previous.logging_out));
                if let Some(old) = &before.check {
                    assert!(!after.has_read(old.request.ticket));
                }
            } else {
                assert!(view == previous && effects.is_empty());
            }
        }
        if matches!(event, Event::ReadFinished(ticket, ..) if !before.has_read(*ticket)) {
            // 旧票正文不能改变当前宿主或打开权限；本轮 Cookie 观测仍可关写。
            assert_eq!(view.epoch, previous.epoch);
            assert_eq!(view.session, previous.session);
            assert!(!view.can_write || previous.can_write);
            assert_eq!(notices, 0);
        }
        if matches!(event, Event::LogoutFinished(..) | Event::RenewalFinished(..))
            && !previous.can_write
            && view.can_write
        {
            // 唯一直接开写的非 GET 是当前首次退出明确拒绝，且发出前有写许可。
            assert!(matches!(event, Event::LogoutFinished(_, _, LogoutOutcome::Rejected(_))));
        }
        self
    }
}
