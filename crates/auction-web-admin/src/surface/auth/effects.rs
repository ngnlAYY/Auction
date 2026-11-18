//! Admin 会话状态机的浏览器事件与副作用适配。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use auction_web_shared::PageVisibility;
use auction_web_shared::clock::monotonic_millis;
use auction_web_shared::storage::read_cookie;
use auction_web_shared::timer::TimeoutFuture;
#[cfg(target_arch = "wasm32")]
use auction_web_shared::visibility::activity::InputActivityListener;
use auction_web_shared::visibility::broadcast::BroadcastNotice;
use dioxus::core::Task;
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use super::activity::LOGOUT_BUTTON_CLASS;
use super::session::{
    Cookie, Effect, Event, LogoutOutcome, ReadOutcome, RenewalOutcome, State, Ticket, Timer, View,
    step,
};
use crate::api::{AdminApiFailure, AdminSession, fetch_admin_session, logout_admin};

pub(super) fn use_session() -> (Signal<View>, EventHandler<Event>) {
    let view = use_signal(View::default);
    let runtime = use_hook(|| Rc::new(SessionRuntime::new(view)));
    let dispatch = {
        let runtime = Rc::clone(&runtime);
        use_callback(move |event| runtime.accept(event))
    };
    use_hook(|| {
        *runtime.broadcast.borrow_mut() =
            BroadcastNotice::open("hv-auction-admin-session", "session-ended", move || {
                dispatch.call(Event::Broadcast);
            });
    });
    use_hook(|| {
        let visibility = PageVisibility::new();
        visibility.install(move || dispatch.call(Event::Visible));
        visibility
    });
    let on_input = move |on_logout| dispatch.call(Event::Input { on_logout });
    #[cfg(target_arch = "wasm32")]
    use_hook(|| {
        InputActivityListener::install(format!(".{LOGOUT_BUTTON_CLASS}"), on_input).map(Rc::new)
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = on_input;
    use_effect(move || dispatch.call(Event::Start));
    (view, dispatch)
}

struct SessionRuntime {
    state: RefCell<State>,
    view: Signal<View>,
    read: Cell<Option<Task>>,
    check_timer: Cell<Option<Task>>,
    renewal_timer: Cell<Option<Task>>,
    broadcast: RefCell<Option<BroadcastNotice>>,
}

impl SessionRuntime {
    fn new(view: Signal<View>) -> Self {
        Self {
            state: RefCell::new(State::default()),
            view,
            read: Cell::new(None),
            check_timer: Cell::new(None),
            renewal_timer: Cell::new(None),
            broadcast: RefCell::new(None),
        }
    }

    fn accept(self: &Rc<Self>, event: Event) {
        let cookie = if matches!(event, Event::Input { .. }) {
            Cookie::Unavailable
        } else {
            cookie_generation()
        };
        let (state, effects) = step(self.state.take(), event, cookie, monotonic_millis());
        let next_view = state.view();
        self.state.replace(state);
        let mut view = self.view;
        if *view.peek() != next_view {
            view.set(next_view);
        }
        for effect in effects {
            self.execute(effect);
        }
    }

    fn execute(self: &Rc<Self>, effect: Effect) {
        match effect {
            Effect::Read(ticket) => self.read(ticket),
            Effect::CancelRead => cancel(&self.read),
            Effect::Logout(ticket, session) => self.logout(ticket, session),
            Effect::Renew(ticket, session) => self.renew(ticket, session),
            Effect::ArmTimer(timer, ticket, delay) => self.arm_timer(timer, ticket, delay),
            Effect::CancelTimer(timer) => cancel(self.timer(timer)),
            Effect::BroadcastEnded => {
                if let Some(notice) = self.broadcast.borrow().as_ref() {
                    notice.notify();
                }
            }
        }
    }

    fn read(self: &Rc<Self>, ticket: Ticket) {
        let runtime = Rc::clone(self);
        self.read.set(Some(spawn(async move {
            let sent = cookie_generation();
            let outcome = match fetch_admin_session().await {
                Ok(Some(session)) => ReadOutcome::Authenticated(Rc::new(session)),
                Ok(None) => ReadOutcome::Unauthenticated,
                Err(error) => ReadOutcome::Failed(error),
            };
            runtime.read.set(None);
            runtime.accept(Event::ReadFinished(ticket, sent, outcome));
        })));
    }

    fn logout(self: &Rc<Self>, ticket: Ticket, session: Rc<AdminSession>) {
        let runtime = Rc::clone(self);
        spawn(async move {
            let sent = cookie_generation();
            let outcome = match logout_admin(session.csrf_token.clone()).await {
                Ok(()) => LogoutOutcome::Succeeded,
                Err(error) => logout_failure(error),
            };
            runtime.accept(Event::LogoutFinished(ticket, sent, outcome));
        });
    }

    fn renew(self: &Rc<Self>, ticket: Ticket, session: Rc<AdminSession>) {
        let runtime = Rc::clone(self);
        let task = spawn(async move {
            let sent = cookie_generation();
            let outcome = match session.renew().await {
                Ok(session) => RenewalOutcome::Succeeded(Rc::new(session)),
                Err(error) => renewal_failure(&error.failure, error.retry_after_millis),
            };
            runtime.accept(Event::RenewalFinished(ticket, sent, outcome));
        });
        // 在定时器事件同一轮启动 fetch。退出点击可以取消定时器，已派发 POST 的回执则仍归外壳。
        let _ = task.poll_now();
    }

    fn timer(&self, timer: Timer) -> &Cell<Option<Task>> {
        match timer {
            Timer::Check => &self.check_timer,
            Timer::Renewal => &self.renewal_timer,
        }
    }

    fn arm_timer(self: &Rc<Self>, timer: Timer, ticket: Ticket, delay: u32) {
        let runtime = Rc::clone(self);
        self.timer(timer).set(Some(spawn(async move {
            if delay > 0 {
                TimeoutFuture::new(delay).await;
            }
            runtime.timer(timer).set(None);
            runtime.accept(Event::TimerElapsed(timer, ticket));
        })));
    }
}

fn cancel(task: &Cell<Option<Task>>) {
    if let Some(task) = task.take() {
        task.cancel();
    }
}

fn cookie_generation() -> Cookie {
    match read_cookie("auction_admin_csrf") {
        Some(Some(value)) => Cookie::Present(Rc::from(value)),
        Some(None) => Cookie::Missing,
        None => Cookie::Unavailable,
    }
}

fn logout_failure(error: AdminApiFailure) -> LogoutOutcome {
    if error.status == Some(429) || error.code == "rate_limited" {
        LogoutOutcome::RateLimited(error.code)
    } else if error.code == "csrf_invalid"
        || matches!(error.status, Some(401 | 408))
        || (error.code != "invalid_request" && error.outcome_unknown())
    {
        LogoutOutcome::Unknown(error.code)
    } else {
        LogoutOutcome::Rejected(error.code)
    }
}

fn renewal_failure(failure: &AdminApiFailure, retry_after: Option<u64>) -> RenewalOutcome {
    match failure.code.as_str() {
        "unauthenticated" if failure.status == Some(401) => RenewalOutcome::Unauthenticated,
        "rate_limited" => RenewalOutcome::RateLimited(retry_after),
        "csrf_invalid" => RenewalOutcome::CsrfRejected,
        "invalid_request" => RenewalOutcome::Rejected,
        _ if failure.status == Some(429) => RenewalOutcome::RateLimited(retry_after),
        _ if matches!(failure.status, Some(401 | 408)) || failure.outcome_unknown() => {
            RenewalOutcome::Unknown
        }
        _ => RenewalOutcome::Rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::{logout_failure, renewal_failure};
    use crate::api::AdminApiFailure;
    use crate::surface::auth::session::{LogoutOutcome, RenewalOutcome};

    fn http(status: u16, code: &str) -> AdminApiFailure {
        AdminApiFailure { status: Some(status), code: code.to_owned() }
    }

    #[test]
    fn logout_failure_requires_a_read_when_session_or_outcome_is_uncertain() {
        for failure in [
            http(403, "csrf_invalid"),
            http(401, "unauthenticated"),
            http(504, "timeout"),
            AdminApiFailure::client("network_error"),
            AdminApiFailure::client("invalid_response"),
        ] {
            assert!(matches!(logout_failure(failure), LogoutOutcome::Unknown(_)));
        }
        assert!(matches!(logout_failure(http(429, "http_429")), LogoutOutcome::RateLimited(_)));
        for failure in [http(403, "origin_forbidden"), AdminApiFailure::client("invalid_request")] {
            assert!(matches!(logout_failure(failure), LogoutOutcome::Rejected(_)));
        }
    }

    #[test]
    fn renewal_failure_classifies_expiration_backoff_and_uncertainty() {
        assert!(matches!(
            renewal_failure(&http(401, "unauthenticated"), None),
            RenewalOutcome::Unauthenticated
        ));
        assert!(matches!(
            renewal_failure(&http(429, "rate_limited"), Some(10_000)),
            RenewalOutcome::RateLimited(Some(10_000))
        ));
        assert!(matches!(
            renewal_failure(&http(403, "csrf_invalid"), None),
            RenewalOutcome::CsrfRejected
        ));
        for failure in [
            http(503, "storage_unavailable"),
            http(504, "http_504"),
            AdminApiFailure::client("network_error"),
            AdminApiFailure::client("request_timeout"),
        ] {
            assert!(matches!(renewal_failure(&failure, None), RenewalOutcome::Unknown));
        }
        for failure in [http(403, "origin_forbidden"), AdminApiFailure::client("invalid_request")] {
            assert!(matches!(renewal_failure(&failure, None), RenewalOutcome::Rejected));
        }
    }
}
