//! 会话资源使用的宿主 HTTP 替身，返回未经解码的响应。

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::future::{pending, poll_fn, ready};
use std::rc::Rc;
use std::task::{Poll, Waker};

use auction_contracts::{
    ApiErrorResponse, ApiResponse, PublicLoginResponse, PublicSetSessionPreferenceRequest,
};
use auction_time::Timestamp;
use dioxus::prelude::*;

use crate::api::{
    HttpFixtureFuture, HttpResponseFixture, PublicSession, SessionHttpFixture, SessionRequest,
    TransportError,
};

type SessionHttpReply = Result<HttpResponseFixture, TransportError>;

#[derive(Clone, Copy)]
pub(in crate::surface) enum Session {
    Rechecking,
    Ready(Option<&'static str>),
}

#[derive(Clone, Copy)]
struct SessionSource(Signal<Session>);

pub(in crate::surface) struct Step {
    request: SessionRequest<'static>,
    reply: HttpFixtureFuture,
}

thread_local! {
    static SCRIPT: RefCell<VecDeque<Step>> = const { RefCell::new(VecDeque::new()) };
    static REQUESTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static PREFERENCE_COMMANDS: RefCell<Vec<PublicSetSessionPreferenceRequest>> = const { RefCell::new(Vec::new()) };
    static LOCK_DEPTH: Cell<usize> = const { Cell::new(0) };
    static SERIALIZE_LOCK: Cell<bool> = const { Cell::new(false) };
    static LOCK_WAITER: RefCell<Option<Waker>> = const { RefCell::new(None) };
    static REVISIONS: RefCell<BTreeMap<String, String>> = const { RefCell::new(BTreeMap::new()) };
    static REVISION_WRITES: RefCell<Vec<(&'static str, bool)>> = const { RefCell::new(Vec::new()) };
    pub(in crate::surface) static FAIL_SESSION_CHECK: Cell<bool> = const { Cell::new(false) };
}

pub(in crate::surface) fn use_session_fixture(user: &'static str) -> Signal<Session> {
    let source = use_signal(|| Session::Ready(Some(user)));
    use_context_provider(|| SessionSource(source));
    use_context_provider(|| SessionHttpFixture {
        respond,
        wait_for_lock: || {
            Box::pin(poll_fn(|context| {
                if SERIALIZE_LOCK.get() && LOCK_DEPTH.get() != 0 {
                    LOCK_WAITER.replace(Some(context.waker().clone()));
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            }))
        },
        track_lock: |acquired| {
            LOCK_DEPTH.update(|depth| if acquired { depth + 1 } else { depth - 1 });
            if !acquired
                && LOCK_DEPTH.get() == 0
                && let Some(waker) = LOCK_WAITER.take()
            {
                waker.wake();
            }
        },
        get_revision: |key| REVISIONS.with_borrow(|values| values.get(key).cloned()),
        set_revision: |key| {
            let key = match key {
                "hv.public.session-revision" => "hv.public.session-revision",
                "hv.public.session-logout-revision" => "hv.public.session-logout-revision",
                "hv.public.session-logout-unknown-revision" => {
                    "hv.public.session-logout-unknown-revision"
                }
                _ => panic!("会话修订键未登记"),
            };
            REVISION_WRITES.with_borrow_mut(|writes| writes.push((key, LOCK_DEPTH.get() > 0)));
            REVISIONS.with_borrow_mut(|values| {
                values.insert(key.to_owned(), uuid::Uuid::new_v4().to_string())
            });
        },
    });
    source
}

pub(in crate::surface) fn reset() {
    SCRIPT.set(VecDeque::new());
    REQUESTS.set(Vec::new());
    PREFERENCE_COMMANDS.set(Vec::new());
    REVISIONS.set(BTreeMap::new());
    REVISION_WRITES.set(Vec::new());
    SERIALIZE_LOCK.set(false);
    LOCK_WAITER.set(None);
    FAIL_SESSION_CHECK.set(false);
}

pub(in crate::surface) fn serialize_cookie_lock() {
    SERIALIZE_LOCK.set(true);
}

pub(in crate::surface) fn take_requests() -> Vec<&'static str> {
    REQUESTS.take()
}

pub(in crate::surface) fn take_preference_commands() -> Vec<PublicSetSessionPreferenceRequest> {
    PREFERENCE_COMMANDS.take()
}

pub(in crate::surface) fn take_revision_writes() -> Vec<(&'static str, bool)> {
    REVISION_WRITES.take()
}

pub(in crate::surface) fn mark_external_logout(key: &str) {
    REVISIONS
        .with_borrow_mut(|values| values.insert(key.to_owned(), uuid::Uuid::new_v4().to_string()));
}

/// 记录请求在写锁之后重新取锁；旧 H1 脚本仍可模拟不经过此锁的外部身份变化。
impl Step {
    pub(in crate::surface) fn under_lock(self) -> Self {
        let Self { request, reply } = self;
        Self {
            request,
            reply: Box::pin(async move {
                assert_eq!(LOCK_DEPTH.get(), 1, "读回应在写锁释放后取得自己的锁");
                reply.await
            }),
        }
    }
}

pub(in crate::surface) fn script(steps: impl IntoIterator<Item = Step>) {
    assert_consumed();
    SCRIPT.set(steps.into_iter().collect());
}

pub(in crate::surface) fn assert_consumed() {
    assert!(SCRIPT.with_borrow(VecDeque::is_empty), "会话请求必须消费全部预期 HTTP 响应");
}

pub(in crate::surface) fn read(user: Option<&'static str>) -> Step {
    reply(SessionRequest::Read, session_response(user))
}

pub(in crate::surface) fn read_finite(user: &str, expires_at: &str) -> Step {
    reply(SessionRequest::Read, session_response_with_deadline(Some(user), Some(expires_at)))
}

pub(in crate::surface) fn reply(request: SessionRequest<'static>, reply: SessionHttpReply) -> Step {
    Step { request, reply: Box::pin(ready(reply)) }
}

pub(in crate::surface) fn rejection(status: u16, code: &str) -> SessionHttpReply {
    Ok(response(
        status,
        serde_json::to_string(&ApiErrorResponse {
            code: code.to_owned(),
            message: "会话请求被拒绝".to_owned(),
            data: None,
            request_id: None,
        })
        .unwrap(),
    ))
}

pub(in crate::surface) fn response(status: u16, body: String) -> HttpResponseFixture {
    HttpResponseFixture { status, etag: None, body: Some(body) }
}

pub(in crate::surface) fn login_success(user_id: &str) -> SessionHttpReply {
    let payload = PublicLoginResponse {
        user_id: user_id.to_owned(),
        username: "bob".to_owned(),
        status: "active".to_owned(),
        expires_at: None,
        absolute_expires_at: None,
    };
    Ok(response(200, serde_json::to_string(&ApiResponse::ok("登录成功", payload)).unwrap()))
}

fn session_response(user: Option<&str>) -> SessionHttpReply {
    session_response_with_deadline(user, None)
}

pub(in crate::surface) fn session_response_with_deadline(
    user: Option<&str>,
    expires_at: Option<&str>,
) -> SessionHttpReply {
    let session = user.map(|user| PublicSession {
        authenticated: true,
        user_id: user.to_owned(),
        username: "alice".to_owned(),
        expires_at: expires_at.map(str::to_owned),
        absolute_expires_at: expires_at.map(str::to_owned),
    });
    Ok(response(200, serde_json::to_string(&ApiResponse::ok("会话读取成功", session)).unwrap()))
}

fn respond(request: SessionRequest<'_>) -> HttpFixtureFuture {
    REQUESTS.with_borrow_mut(|requests| requests.push(request_key(request).0));
    if let SessionRequest::SetPreference(_, command) = request {
        PREFERENCE_COMMANDS.with_borrow_mut(|commands| commands.push(command.clone()));
    }
    if let Some(step) = SCRIPT.with_borrow_mut(VecDeque::pop_front) {
        assert_eq!(
            request_key(request),
            request_key(step.request),
            "会话请求的方法与预期用户必须匹配"
        );
        return step.reply;
    }
    assert!(
        matches!(request, SessionRequest::Read | SessionRequest::Sync(_)),
        "Cookie 变更必须提供响应脚本"
    );
    if FAIL_SESSION_CHECK.get() {
        return Box::pin(ready(Err(TransportError::Network(None))));
    }
    match (consume_context::<SessionSource>().0)() {
        Session::Ready(user) => Box::pin(ready(session_response(user))),
        Session::Rechecking => Box::pin(pending()),
    }
}

fn request_key(request: SessionRequest<'_>) -> (&'static str, Option<&str>) {
    match request {
        SessionRequest::Read => ("GET", None),
        SessionRequest::Sync(user_id) => ("PUT", Some(user_id)),
        SessionRequest::Login(_) => ("POST", None),
        SessionRequest::Logout(user_id) => ("DELETE", Some(user_id)),
        SessionRequest::ReadPreference => ("GET preference", None),
        SessionRequest::SetPreference(user_id, _) => ("PUT preference", Some(user_id)),
    }
}

#[derive(Clone)]
pub(in crate::surface) struct ExpiryClock {
    state: Rc<ExpiryClockState>,
}

struct ExpiryClockState {
    now: Cell<Timestamp>,
    waker: RefCell<Option<Waker>>,
}

impl ExpiryClock {
    pub(in crate::surface) fn new(now: Timestamp) -> Self {
        Self { state: Rc::new(ExpiryClockState { now: Cell::new(now), waker: RefCell::new(None) }) }
    }

    pub(in crate::surface) fn now(&self) -> Timestamp {
        self.state.now.get()
    }

    pub(in crate::surface) async fn wait(&self, _millis: u32) {
        let started = self.now();
        poll_fn(|context| {
            if self.now() != started {
                Poll::Ready(())
            } else {
                self.state.waker.replace(Some(context.waker().clone()));
                Poll::Pending
            }
        })
        .await;
    }

    pub(in crate::surface) fn waiting(&self) -> bool {
        self.state.waker.borrow().is_some()
    }

    pub(in crate::surface) fn advance_to(&self, now: Timestamp) {
        self.state.now.set(now);
        if let Some(waker) = self.state.waker.take() {
            waker.wake();
        }
    }
}

#[derive(Default)]
pub(in crate::surface) struct ResponseGate {
    released: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl ResponseGate {
    pub(in crate::surface) fn hold_read(self: &Rc<Self>, user: Option<&str>) -> Step {
        self.hold(SessionRequest::Read, session_response(user))
    }

    /// 模拟响应尚未到达的请求，以断言收到身份后立即销毁。
    pub(in crate::surface) fn hold(
        self: &Rc<Self>,
        request: SessionRequest<'static>,
        reply: SessionHttpReply,
    ) -> Step {
        let gate = self.clone();
        let reply = Box::pin(async move {
            gate.wait().await;
            reply
        });
        Step { request, reply }
    }

    pub(in crate::surface) async fn wait(&self) {
        poll_fn(|context| {
            if self.released.get() {
                Poll::Ready(())
            } else {
                self.waker.replace(Some(context.waker().clone()));
                Poll::Pending
            }
        })
        .await;
    }

    pub(in crate::surface) fn release(&self) {
        self.released.set(true);
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    pub(in crate::surface) fn waiting(&self) -> bool {
        self.waker.borrow().is_some()
    }
}
