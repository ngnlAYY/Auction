//! 会话资源使用的宿主 HTTP 替身，返回未经解码的响应。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::{pending, poll_fn, ready};
use std::rc::Rc;
use std::task::{Poll, Waker};

use auction_contracts::{ApiErrorResponse, ApiResponse, PublicLoginResponse};
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
    pub(in crate::surface) static FAIL_SESSION_CHECK: Cell<bool> = const { Cell::new(false) };
}

pub(in crate::surface) fn use_session_fixture(user: &'static str) -> Signal<Session> {
    let source = use_signal(|| Session::Ready(Some(user)));
    use_context_provider(|| SessionSource(source));
    use_context_provider(|| SessionHttpFixture(respond));
    source
}

pub(in crate::surface) fn reset() {
    SCRIPT.set(VecDeque::new());
    FAIL_SESSION_CHECK.set(false);
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
    let session = user.map(|user| PublicSession {
        authenticated: true,
        user_id: user.to_owned(),
        username: "alice".to_owned(),
        expires_at: None,
        absolute_expires_at: None,
    });
    Ok(response(200, serde_json::to_string(&ApiResponse::ok("会话读取成功", session)).unwrap()))
}

fn respond(request: SessionRequest<'_>) -> HttpFixtureFuture {
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
    }
}

#[derive(Default)]
pub(in crate::surface) struct ResponseGate {
    released: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl ResponseGate {
    /// 模拟响应尚未到达的请求，以断言收到身份后立即销毁。
    pub(in crate::surface) fn hold(
        self: &Rc<Self>,
        request: SessionRequest<'static>,
        reply: SessionHttpReply,
    ) -> Step {
        let gate = self.clone();
        let reply = Box::pin(async move {
            poll_fn(|context| {
                if gate.released.get() {
                    Poll::Ready(())
                } else {
                    gate.waker.replace(Some(context.waker().clone()));
                    Poll::Pending
                }
            })
            .await;
            reply
        });
        Step { request, reply }
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
