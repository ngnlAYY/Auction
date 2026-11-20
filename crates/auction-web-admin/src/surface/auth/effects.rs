//! Admin 会话状态机的浏览器事件与副作用适配。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use auction_web_shared::PageVisibility;
use auction_web_shared::clock::monotonic_millis;
use auction_web_shared::timer::TimeoutFuture;
#[cfg(target_arch = "wasm32")]
use auction_web_shared::visibility::activity::InputActivityListener;
use auction_web_shared::visibility::broadcast::BroadcastNotice;
use auction_web_shared::visibility::document_is_visible;
use dioxus::core::Task;
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use super::activity::LOGOUT_BUTTON_CLASS;
use super::login::LoginSubmission;
use super::session::{
    Cookie, Effect, Event, LogoutOutcome, ReadOutcome, RenewalOutcome, State, Ticket, Timer, View,
    step,
};
use crate::api::{
    AdminApiFailure, AdminSession, admin_session_cookie, fetch_admin_session, login_admin,
    logout_admin,
};

/// 宿主测试注入会话投影和事件，继续通过真实 Router 与认证外壳挂载业务页面。
#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) struct TestSession(pub(super) Signal<View>, pub(super) EventHandler<Event>);

pub(super) fn use_session() -> (Signal<View>, EventHandler<Event>, EventHandler<LoginSubmission>) {
    #[cfg(test)]
    let fixture = try_consume_context::<TestSession>();
    let view = use_signal(View::default);
    let runtime = use_hook(|| Rc::new(SessionRuntime::new(view)));
    let dispatch = {
        let runtime = Rc::clone(&runtime);
        use_callback(move |event| runtime.accept(event))
    };
    let submit_login = {
        let runtime = Rc::clone(&runtime);
        use_callback(move |submission| runtime.login(submission))
    };
    use_hook(|| {
        *runtime.broadcast.borrow_mut() =
            BroadcastNotice::open("hv-auction-admin-session", "session-ended", move || {
                dispatch.call(Event::Broadcast);
            });
    });
    use_hook(|| {
        let visibility = PageVisibility::new();
        visibility.install(move || on_visible(dispatch));
        visibility
    });
    let on_input = move |on_logout| dispatch.call(Event::Input { on_logout });
    #[cfg(target_arch = "wasm32")]
    use_hook(|| {
        InputActivityListener::install(format!(".{LOGOUT_BUTTON_CLASS}"), on_input).map(Rc::new)
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = on_input;
    use_effect(move || {
        #[cfg(test)]
        if fixture.is_some() {
            return;
        }
        dispatch.call(Event::Start);
    });
    #[cfg(test)]
    if let Some(TestSession(view, dispatch)) = fixture {
        return (view, dispatch, submit_login);
    }
    (view, dispatch, submit_login)
}

fn on_visible(dispatch: EventHandler<Event>) {
    dispatch.call(Event::Visible);
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
        self.accept_with_cookie(event, cookie);
    }

    fn accept_with_cookie(self: &Rc<Self>, event: Event, cookie: Cookie) {
        let (state, effects) = step(self.state.take(), event, cookie, monotonic_millis());
        self.state.replace(state);
        self.publish();
        for effect in effects {
            self.execute(effect);
        }
    }

    fn publish(&self) {
        let next_view = self.state.borrow().view();
        let mut view = self.view;
        if *view.peek() != next_view {
            view.set(next_view);
        }
    }

    fn login(self: &Rc<Self>, submission: LoginSubmission) {
        let Some(ticket) = self.state.borrow_mut().start_login() else { return };
        self.publish();
        let runtime = Rc::clone(self);
        // 外壳作用域持有已经派发的 POST；表单被卸载时也要消费回执并复核当前 Cookie。
        let task = spawn(async move {
            let outcome = request_login(submission).await.map(Rc::new);
            runtime.accept(Event::LoginFinished(ticket, outcome));
        });
        let _ = task.poll_now();
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
            if !runtime.state.borrow().has_read(ticket) {
                return;
            }
            if runtime
                .state
                .borrow_mut()
                .cancel_discovery_before_send(ticket, document_is_visible())
            {
                runtime.read.set(None);
                return;
            }
            let sent = cookie_generation();
            // 复核等待网络前立即发布这次 Cookie 观测，撤回旧身份的写许可和 Key 展示。
            runtime.accept_with_cookie(Event::ReadSending(ticket), sent.clone());
            if !runtime.state.borrow().has_read(ticket) {
                return;
            }
            let outcome = match request_session().await {
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
            runtime.accept_with_cookie(Event::LogoutSending(ticket), sent.clone());
            if !runtime.state.borrow().has_logout(ticket) {
                return;
            }
            let result = logout_admin(session.csrf_token.clone()).await;
            runtime.accept(logout_completion(ticket, sent, result));
        });
    }

    fn renew(self: &Rc<Self>, ticket: Ticket, session: Rc<AdminSession>) {
        let runtime = Rc::clone(self);
        let task = spawn(async move {
            let sent = cookie_generation();
            if !runtime.prepare_renewal_send(ticket, &sent) {
                return;
            }
            let outcome = match session.renew().await {
                Ok(session) => RenewalOutcome::Succeeded(Rc::new(session)),
                Err(error) => renewal_failure(&error.failure, error.retry_after_millis),
            };
            runtime.accept(Event::RenewalFinished(ticket, sent, outcome));
        });
        // 在定时器事件同一轮启动 fetch。退出点击可以取消定时器，已派发 POST 的回执则仍归外壳。
        let _ = task.poll_now();
    }

    fn prepare_renewal_send(self: &Rc<Self>, ticket: Ticket, sent: &Cookie) -> bool {
        self.accept_with_cookie(Event::RenewalSending(ticket), sent.clone());
        self.state.borrow().has_renewal(ticket)
    }

    fn timer(&self, timer: Timer) -> &Cell<Option<Task>> {
        match timer {
            Timer::Check => &self.check_timer,
            Timer::Renewal => &self.renewal_timer,
        }
    }

    fn arm_timer(self: &Rc<Self>, timer: Timer, ticket: Ticket, delay: u32) {
        #[cfg(test)]
        if try_consume_context::<Rc<tests::SessionTransport>>().is_some() {
            // 宿主替身只推进显式回执，避免启动浏览器定时器。
            return;
        }
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

async fn request_login(submission: LoginSubmission) -> Result<AdminSession, String> {
    #[cfg(test)]
    if let Some(transport) = try_consume_context::<Rc<tests::SessionTransport>>() {
        return transport.login(submission).await;
    }
    login_admin(submission.actor, submission.secret).await
}

async fn request_session() -> Result<Option<AdminSession>, String> {
    #[cfg(test)]
    if let Some(transport) = try_consume_context::<Rc<tests::SessionTransport>>() {
        return transport.get.request().await;
    }
    fetch_admin_session().await
}

fn cancel(task: &Cell<Option<Task>>) {
    if let Some(task) = task.take() {
        task.cancel();
    }
}

fn cookie_generation() -> Cookie {
    match admin_session_cookie() {
        Some(Some(value)) => Cookie::Present(Rc::from(value)),
        Some(None) => Cookie::Missing,
        None => Cookie::Unavailable,
    }
}

fn logout_completion(ticket: Ticket, sent: Cookie, result: Result<(), AdminApiFailure>) -> Event {
    // 明确在构造请求或传输准备时失败的重试，仍由先前实际发送的票持有通知证据。
    let dispatched = result.as_ref().err().is_none_or(|error| {
        error.status.is_some() || !AdminApiFailure::definite_rejection(&error.code)
    });
    let outcome = match result {
        Ok(()) => LogoutOutcome::Succeeded,
        Err(error) => logout_failure(error),
    };
    if dispatched {
        Event::LogoutFinished(ticket, sent, outcome)
    } else {
        Event::LogoutNotSent(ticket, sent, outcome)
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
    use std::cell::{Cell, RefCell};
    use std::future::poll_fn;
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    use auction_time::{Duration, Timestamp};
    use dioxus::core::Mutation;
    use dioxus::history::MemoryHistory;
    use dioxus::prelude::*;
    use dioxus::router::RouterConfig;

    use super::{
        SessionRuntime, TestSession, logout_completion, logout_failure, on_visible,
        renewal_failure, use_session,
    };
    use crate::api::{AdminApiFailure, AdminSession};
    use crate::surface::auctions::draft_page_tests::{
        button, click_button, has_live_text, init_i18n, mount_writer, settle_writer, writer_form,
    };
    use crate::surface::auth::login::{AdminLogin, LoginSubmission};
    use crate::surface::auth::session::{
        Cookie, Effect, Event, LogoutOutcome, ReadOutcome, RenewalOutcome, State, Timer, View, step,
    };
    use crate::surface::router::Route;

    pub(super) struct ResponseStub<T> {
        calls: Cell<usize>,
        result: RefCell<Option<T>>,
        waker: RefCell<Option<Waker>>,
    }

    impl<T> Default for ResponseStub<T> {
        fn default() -> Self {
            Self { calls: Cell::new(0), result: RefCell::new(None), waker: RefCell::new(None) }
        }
    }

    impl<T> ResponseStub<T> {
        pub(super) async fn request(&self) -> T {
            self.calls.set(self.calls.get() + 1);
            poll_fn(|cx| {
                if let Some(result) = self.result.take() {
                    Poll::Ready(result)
                } else {
                    self.waker.replace(Some(cx.waker().clone()));
                    Poll::Pending
                }
            })
            .await
        }

        fn finish(&self, result: T) {
            let wake = self.waker.take().expect("请求已经派发并等待回执");
            assert!(self.result.replace(Some(result)).is_none());
            wake.wake();
        }
    }

    #[derive(Default)]
    pub(super) struct SessionTransport {
        post: ResponseStub<Result<AdminSession, String>>,
        pub(super) get: ResponseStub<Result<Option<AdminSession>, String>>,
    }

    impl SessionTransport {
        pub(super) async fn login(
            &self,
            submission: LoginSubmission,
        ) -> Result<AdminSession, String> {
            assert_eq!(submission.actor, "fixture-operator");
            assert_eq!(submission.secret, "fixture-password");
            self.post.request().await
        }
    }

    fn fixture_session() -> AdminSession {
        let now = Timestamp::parse_from_rfc3339("2026-11-18T00:00:00Z").unwrap();
        AdminSession {
            actor: "fixture-operator".to_owned(),
            csrf_token: "fixture-cookie".to_owned(),
            expires_at: now.checked_add_signed(Duration::minutes(15)).unwrap(),
            absolute_expires_at: now.checked_add_signed(Duration::hours(8)).unwrap(),
            server_time: now,
            received_at_millis: 0.0,
        }
    }

    #[test]
    fn activity_send_gate_observes_cookie_and_hides_key_before_recheck() {
        let session = Rc::new(fixture_session());
        let current = Cookie::Present(Rc::from(session.csrf_token.as_str()));
        let (state, effects) = step(State::default(), Event::Start, current.clone(), 0.0);
        let [Effect::Read(read)] = effects.as_slice() else { panic!("应读取初始会话") };
        let (state, _) = step(
            state,
            Event::ReadFinished(*read, current.clone(), ReadOutcome::Authenticated(session)),
            current.clone(),
            0.0,
        );
        let (state, effects) = step(state, Event::Input { on_logout: false }, current.clone(), 0.0);
        let [Effect::ArmTimer(Timer::Renewal, timer, delay)] = effects.as_slice() else {
            panic!("应安排一次续期");
        };
        let (state, effects) = step(
            state,
            Event::TimerElapsed(Timer::Renewal, *timer),
            current.clone(),
            f64::from(*delay),
        );
        let [Effect::Renew(ticket, _)] = effects.as_slice() else {
            panic!("应登记一次续期")
        };
        let ticket = *ticket;

        for (observed, may_send) in [
            (current, true),
            (Cookie::Present(Rc::from("replacement-cookie")), false),
            (Cookie::Missing, false),
        ] {
            let transport = Rc::new(SessionTransport::default());
            let sent = Rc::new(Cell::new(None));
            let (mut page, _) = mount_writer({
                let transport = transport.clone();
                let sent = sent.clone();
                let state = state.clone();
                move || {
                    use_context_provider(|| transport.clone());
                    let view = use_signal(|| state.view());
                    let runtime = use_hook(|| {
                        let runtime = Rc::new(SessionRuntime::new(view));
                        runtime.state.replace(state.clone());
                        runtime
                    });
                    let observed = observed.clone();
                    let sent = sent.clone();
                    rsx! {
                        button { onclick: move |_| {
                            let previous = view();
                            assert!(previous.can_write && !previous.key_visibility.hidden);
                            sent.set(Some(runtime.prepare_renewal_send(ticket, &observed)));
                            let current = view();
                            assert_eq!(current.can_write, may_send);
                            assert_eq!(current.key_visibility.hidden, !may_send);
                            assert_eq!(current.session, previous.session);
                        }, {"尝试实发活动续期".to_owned()} }
                    }
                }
            });
            click_button(&mut page, "尝试实发活动续期");
            assert_eq!(sent.get(), Some(may_send));
            assert_eq!(transport.get.calls.get(), usize::from(!may_send));
        }
    }

    #[component]
    fn DiscoveryShell(view: Signal<View>, dispatch: EventHandler<Event>) -> Element {
        use_context_provider(|| TestSession(view, dispatch));
        rsx! {
            Router::<Route> { config: move |_| RouterConfig::<Route>::default().on_update(|router| {
                matches!(router.current(), Route::Dashboard {}).then(|| Route::Status {}.into())
            }) }
        }
    }

    #[test]
    fn visible_logged_out_shell_reads_once_then_enters_admin() {
        let transport = Rc::new(SessionTransport::default());
        let history = Rc::new(MemoryHistory::with_initial_path(Route::Login {}));
        let snapshot = Rc::new(Cell::new(None::<Signal<View>>));
        let (mut page, _) = mount_writer({
            let transport = transport.clone();
            let history = history.clone();
            let snapshot = snapshot.clone();
            move || {
                init_i18n();
                use_context_provider(|| transport.clone());
                use_context_provider(|| history.clone() as Rc<dyn History>);
                let (view, dispatch, _) = use_session();
                snapshot.set(Some(view));
                rsx! {
                    button { onclick: move |_| on_visible(dispatch), {"恢复可见".to_owned()} }
                    DiscoveryShell { view, dispatch }
                }
            }
        });
        assert_eq!(transport.get.calls.get(), 1);
        transport.get.finish(Ok(None));
        settle_writer(&mut page);
        assert_eq!(history.current_route(), "/login");
        assert!(has_live_text(&page, "后台登录"));

        click_button(&mut page, "恢复可见");
        click_button(&mut page, "恢复可见");
        assert_eq!(transport.get.calls.get(), 2);
        assert_eq!(transport.post.calls.get(), 0);
        assert!(has_live_text(&page, "后台登录"));
        transport.get.finish(Ok(Some(fixture_session())));
        settle_writer(&mut page);
        settle_writer(&mut page);
        assert_eq!(transport.get.calls.get(), 2);
        assert!(snapshot.get().unwrap().peek().can_write);
        assert!(button(&page, "退出登录").is_some());
        assert!(!has_live_text(&page, "后台登录"));
        assert_eq!(history.current_route(), "/status");
    }

    fn verify_broadcast_login(failure: Option<(&str, &str)>) {
        let transport = Rc::new(SessionTransport::default());
        let snapshot = Rc::new(Cell::new(None::<Signal<View>>));
        let (mut page, _) = mount_writer({
            let transport = transport.clone();
            let snapshot = snapshot.clone();
            move || {
                init_i18n();
                use_context_provider(|| transport.clone());
                let (view, dispatch, submit_login) = use_session();
                snapshot.set(Some(view));
                let current = view();
                rsx! {
                    button { onclick: move |_| dispatch.call(Event::Broadcast), {"广播结束会话".to_owned()} }
                    if current.logged_out {
                        AdminLogin {
                            submitting: current.login_pending,
                            login_error: current.login_error,
                            on_edit: move |_| dispatch.call(Event::LoginInputChanged),
                            on_submit: submit_login,
                        }
                    }
                }
            }
        });
        assert_eq!(transport.get.calls.get(), 1);
        transport.get.finish(Ok(None));
        let edits = settle_writer(&mut page);
        let inputs: Vec<_> = edits
            .iter()
            .filter_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "input" => Some(*id),
                _ => None,
            })
            .collect();
        let [actor, secret] = inputs.as_slice() else { panic!("应有两个登录输入") };
        page.dispatch("input", *actor, "fixture-operator");
        page.dispatch("input", *secret, "fixture-password");
        page.dispatch("submit", writer_form(&edits), "");
        assert_eq!(transport.post.calls.get(), 1);
        assert!(has_live_text(&page, "登录中…"));

        click_button(&mut page, "广播结束会话");
        click_button(&mut page, "广播结束会话");
        let view = snapshot.get().unwrap();
        assert!(!view.peek().logged_out);
        assert!(!has_live_text(&page, "登录中…"));
        assert_eq!(transport.get.calls.get(), 1);

        let now = Timestamp::parse_from_rfc3339("2026-11-18T00:00:00Z").unwrap();
        let session = AdminSession {
            actor: "fixture-operator".to_owned(),
            csrf_token: "fixture-cookie".to_owned(),
            expires_at: now.checked_add_signed(Duration::minutes(15)).unwrap(),
            absolute_expires_at: now.checked_add_signed(Duration::hours(8)).unwrap(),
            server_time: now,
            received_at_millis: 0.0,
        };
        transport
            .post
            .finish(failure.map_or_else(|| Ok(session.clone()), |(code, _)| Err(code.to_owned())));
        page.rerender();
        assert_eq!(transport.get.calls.get(), 2);
        assert!(!view.peek().can_write);
        assert!(!view.peek().logged_out);
        transport.get.finish(Ok(failure.is_none().then_some(session)));
        let edits = settle_writer(&mut page);
        assert_eq!(transport.get.calls.get(), 2);
        assert_eq!(transport.post.calls.get(), 1);
        assert_eq!(view.peek().can_write, failure.is_none());
        if let Some((code, message)) = failure {
            assert!(view.peek().logged_out);
            assert_eq!(view.peek().login_error.as_deref(), Some(code));
            assert!(has_live_text(&page, message));
            let actor = edits
                .iter()
                .find_map(|edit| match edit {
                    Mutation::NewEventListener { name, id } if name == "input" => Some(*id),
                    _ => None,
                })
                .unwrap();
            page.dispatch("input", actor, "fixture-new-input");
            assert!(view.peek().login_error.is_none());
            assert!(!has_live_text(&page, message));
        }
    }

    #[test]
    fn login_success_after_broadcast_waits_for_one_cookie_read() {
        verify_broadcast_login(None);
    }

    #[test]
    fn login_rejection_after_broadcast_is_shown_by_the_remounted_form() {
        verify_broadcast_login(Some(("invalid_credentials", "用户名或密码无效。")));
    }

    #[test]
    fn unknown_login_outcome_after_broadcast_reads_once_and_never_reposts() {
        verify_broadcast_login(Some(("request_timeout", "操作结果尚不确定，人工核对前请勿重试。")));
    }

    fn http(status: u16, code: &str) -> AdminApiFailure {
        let body = serde_json::json!({
            "code": code, "message": "测试回执", "data": null, "request_id": null
        });
        AdminApiFailure::from_http_bytes(status, body.to_string().as_bytes())
    }

    #[test]
    fn logout_completion_distinguishes_unsent_from_sent_delete() {
        let ticket = 1;
        let cookie = Cookie::Present(Rc::from("fixture-cookie"));
        for (failure, was_sent, expected_code) in [
            (AdminApiFailure::client("invalid_request"), false, "invalid_request"),
            (http(403, "origin_forbidden"), true, "origin_forbidden"),
        ] {
            let (dispatched, observed_ticket, sent, code) =
                match logout_completion(ticket, cookie.clone(), Err(failure)) {
                    Event::LogoutNotSent(ticket, sent, LogoutOutcome::Rejected(code)) => {
                        (false, ticket, sent, code)
                    }
                    Event::LogoutFinished(ticket, sent, LogoutOutcome::Rejected(code)) => {
                        (true, ticket, sent, code)
                    }
                    _ => panic!("应收到明确拒绝的退出回执"),
                };
            assert_eq!(dispatched, was_sent);
            assert_eq!(observed_ticket, ticket);
            assert!(sent == cookie);
            assert_eq!(code, expected_code);
        }
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

    #[test]
    fn raw_renewal_receipt_requires_both_trusted_body_and_401_status() {
        let valid =
            br#"{"code":"unauthenticated","message":"expired","data":null,"request_id":null}"#;
        let truncated = br#"{"code":"unauthenticated","message":"expired","data":null}"#;
        assert!(matches!(
            renewal_failure(&AdminApiFailure::from_http_bytes(401, valid), None),
            RenewalOutcome::Unauthenticated
        ));
        for (status, body) in [(401, truncated.as_slice()), (503, valid.as_slice())] {
            assert!(matches!(
                renewal_failure(&AdminApiFailure::from_http_bytes(status, body), None),
                RenewalOutcome::Unknown
            ));
        }
    }
}
