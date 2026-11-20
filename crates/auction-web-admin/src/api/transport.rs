//! Admin 同源请求发送与写前 CSRF 复核。
//! 传输边界复用 `auction-web-shared` 的统一超时与错误分类；业务写入在本层核实回执来源。
//! Admin 请求的认证头在本层或非命令调用方装配，共享层不感知凭据。

use std::{borrow::Cow, ops::Deref};

use auction_web_shared::TransportError;
use dioxus::prelude::try_consume_context;
use gloo_net::http::{Method, Request, RequestBuilder, Response};

use super::failure::{
    AdminApiFailure, AdminWriteEndpoint, AdminWriteFailure, DecodedHttpFailure,
    invalid_request_error, valid_protocol_code,
};
use super::session::{
    AdminSession, AdminSessionInvalidation, AdminSessionWriteReview, admin_session_writes_allowed,
    fetch_admin_session, request_admin_session_check,
};

/// 同源响应及其发出请求时的会话归属，仅 HTTP 401 需要保留归属。
pub(crate) struct AdminResponse {
    #[cfg(not(test))]
    response: Response,
    #[cfg(test)]
    response: TestResponse,
    invalidation: Option<AdminSessionInvalidation>,
}

impl AdminResponse {
    fn new(response: Response, invalidation: Option<AdminSessionInvalidation>) -> Self {
        let invalidation = if response.status() == 401 { invalidation } else { None };
        #[cfg(test)]
        let response = TestResponse::Browser(response);
        Self { response, invalidation }
    }

    /// 仅由错误解码器在确认收到可信的 401 后调用。
    pub(super) fn report_session_invalidation(self) {
        if let Some(invalidation) = self.invalidation {
            invalidation.report();
        }
    }
}

#[cfg(not(test))]
impl Deref for AdminResponse {
    type Target = Response;

    fn deref(&self) -> &Response {
        &self.response
    }
}

// 宿主测试直接提供成功回执的正文，仍由实际 API 调用方解码并核对。
#[cfg(test)]
enum TestResponse {
    Browser(Response),
    Json(Cow<'static, [u8]>),
}

#[cfg(test)]
impl AdminResponse {
    fn from_stub_json(bytes: Cow<'static, [u8]>) -> Self {
        Self { response: TestResponse::Json(bytes), invalidation: None }
    }

    /// 用与浏览器响应相同的 serde 解码读取替身正文。
    pub(crate) async fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, gloo_net::Error> {
        match &self.response {
            TestResponse::Browser(response) => response.json().await,
            TestResponse::Json(bytes) => Ok(serde_json::from_slice(bytes)?),
        }
    }
}

#[cfg(test)]
impl Deref for AdminResponse {
    type Target = Response;

    fn deref(&self) -> &Response {
        match &self.response {
            TestResponse::Browser(response) => response,
            TestResponse::Json(_) => panic!("成功替身只提供 JSON 正文"),
        }
    }
}

/// 发送不改写 URL 的 Admin GET 请求。
pub(crate) async fn send_get(url: &str) -> Result<AdminResponse, String> {
    let invalidation = AdminSessionInvalidation::capture();
    let response = auction_web_shared::send_get(url).await.map_err(transport_error_code)?;
    Ok(AdminResponse::new(response, invalidation))
}

/// 在共享传输层的统一超时上限内发送一次 Admin 请求。
/// 保存构建阶段：只有成功构造请求、交给 fetch 后的故障可能对应已提交的业务写入。
pub(crate) async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
) -> Result<AdminResponse, String> {
    send_request_with_timeout(build, auction_web_shared::DEFAULT_REQUEST_TIMEOUT_MILLIS).await
}

/// 经过实际业务写请求及错误信封校验的 HTTP 回执。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedWriteReceipt {
    status: u16,
    endpoint: AdminWriteEndpoint,
    protocol_code: String,
}

impl VerifiedWriteReceipt {
    fn new(status: u16, endpoint: AdminWriteEndpoint, protocol_code: String) -> Self {
        Self { status, endpoint, protocol_code }
    }

    /// 实际写响应的状态码。
    pub(crate) const fn status(&self) -> u16 {
        self.status
    }

    /// 实际发出请求的端点类别。
    pub(crate) const fn endpoint(&self) -> AdminWriteEndpoint {
        self.endpoint
    }

    /// 通过信封校验的原协议码。
    pub(crate) fn protocol_code(&self) -> &str {
        &self.protocol_code
    }
}

/// 绑定一次业务写入的方法、地址与回执类别。
pub(super) struct WriteRequest<'a> {
    url: &'a str,
    method: Method,
    endpoint: AdminWriteEndpoint,
}

enum WriteBody<'a, T> {
    Json(&'a T),
    Frozen(&'a str),
}

impl<'a, T: serde::Serialize> WriteBody<'a, T> {
    fn encode(self) -> Result<Cow<'a, str>, serde_json::Error> {
        match self {
            Self::Json(command) => serde_json::to_string(command).map(Cow::Owned),
            Self::Frozen(body) => Ok(Cow::Borrowed(body)),
        }
    }
}

#[cfg(not(test))]
type BrowserWriteResponse = AdminResponse;
// 只有测试枚举需要同时容纳小得多的替身回执，浏览器构建仍直接保存响应。
#[cfg(test)]
type BrowserWriteResponse = Box<AdminResponse>;

enum WriteResponse {
    Browser(BrowserWriteResponse),
    #[cfg(test)]
    Stub {
        status: u16,
        bytes: Cow<'static, [u8]>,
    },
}

impl From<AdminResponse> for WriteResponse {
    fn from(response: AdminResponse) -> Self {
        #[cfg(test)]
        let response = Box::new(response);
        Self::Browser(response)
    }
}

impl<'a> WriteRequest<'a> {
    /// 无端点专属拒绝规则的业务写请求。
    pub(super) fn general(url: &'a str, method: Method) -> Self {
        Self::new(url, method, AdminWriteEndpoint::General)
    }

    /// 创建按端点裁定回执的业务写请求。
    pub(super) fn new(url: &'a str, method: Method, endpoint: AdminWriteEndpoint) -> Self {
        Self { url, method, endpoint }
    }

    /// 使用原 JSON 编码路径发送命令。
    pub(super) async fn send_json(
        self,
        command: &impl serde::Serialize,
    ) -> Result<AdminResponse, AdminWriteFailure> {
        self.send(auction_web_shared::DEFAULT_REQUEST_TIMEOUT_MILLIS, WriteBody::Json(command))
            .await
    }

    /// 原样发送冻结的 JSON 字节，允许调用方保留专用预算。
    pub(super) async fn send_body_with_timeout(
        self,
        body: &str,
        timeout_millis: u32,
    ) -> Result<AdminResponse, AdminWriteFailure> {
        self.send(timeout_millis, WriteBody::<()>::Frozen(body)).await
    }

    /// 在默认预算内原样发送冻结的 JSON 字节。
    pub(super) async fn send_body(self, body: &str) -> Result<AdminResponse, AdminWriteFailure> {
        self.send_body_with_timeout(body, auction_web_shared::DEFAULT_REQUEST_TIMEOUT_MILLIS).await
    }

    async fn send(
        self,
        timeout_millis: u32,
        body: WriteBody<'_, impl serde::Serialize>,
    ) -> Result<AdminResponse, AdminWriteFailure> {
        if !matches!(self.method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
            return Err(AdminWriteFailure::not_sent("invalid_request"));
        }
        let csrf_token = review_write_session().await.map_err(AdminWriteFailure::not_sent)?;
        let endpoint = self.endpoint;
        let response = self
            .dispatch(&csrf_token, timeout_millis, body)
            .await
            .map_err(RequestFailure::into_write_failure)?;
        let (response, status, body) = match response {
            WriteResponse::Browser(response) => {
                #[cfg(test)]
                let response = *response;
                if response.ok() {
                    return Ok(response);
                }
                let status = response.status();
                let body = response.binary().await.map(Cow::<[u8]>::Owned);
                (Some(response), status, body)
            }
            #[cfg(test)]
            WriteResponse::Stub { status, bytes } if (200..300).contains(&status) => {
                return Ok(AdminResponse::from_stub_json(bytes));
            }
            #[cfg(test)]
            WriteResponse::Stub { status, bytes } => (None, status, Ok(bytes)),
        };
        let decoded = match body {
            Ok(bytes) => DecodedHttpFailure::from_bytes(status, &bytes),
            // 网络正文中断已在共享传输层返回。这里读的是已缓冲的浏览器响应。
            Err(_error) => DecodedHttpFailure::invalid_body(status),
        };
        if decoded.status == 401
            && decoded.protocol_code.as_deref() == Some("unauthenticated")
            && let Some(response) = response
        {
            response.report_session_invalidation();
        }
        Err(Self::write_failure(endpoint, decoded))
    }

    async fn dispatch(
        self,
        csrf_token: &str,
        timeout_millis: u32,
        body: WriteBody<'_, impl serde::Serialize>,
    ) -> Result<WriteResponse, RequestFailure> {
        #[cfg(test)]
        if let Some(stub) = try_consume_context::<WriteTransportStub>() {
            let body = body.encode().map_err(|error| RequestFailure {
                built: false,
                error: TransportError::Network(None),
                build_failure: Some(invalid_request_error(error)),
            })?;
            return (stub.dispatch)(self.url, &self.method, body.as_bytes(), timeout_millis)
                .into_response();
        }
        dispatch_request(
            |abort_signal| {
                let builder = RequestBuilder::new(self.url)
                    .method(self.method)
                    .abort_signal(abort_signal)
                    .header("x-csrf-token", csrf_token);
                let body = body.encode().map_err(invalid_request_error)?;
                builder
                    .header("Content-Type", "application/json")
                    .body(body.as_ref())
                    .map_err(invalid_request_error)
            },
            timeout_millis,
        )
        .await
        .map(WriteResponse::from)
    }

    // 只有实际写入的发送器能封装回执。畸形正文或伪内部码都不能解除更早的未知。
    fn write_failure(
        endpoint: AdminWriteEndpoint,
        decoded: DecodedHttpFailure,
    ) -> AdminWriteFailure {
        let DecodedHttpFailure { status, protocol_code } = decoded;
        match protocol_code {
            Some(protocol_code) if valid_protocol_code(&protocol_code) => {
                AdminWriteFailure::VerifiedHttp(VerifiedWriteReceipt::new(
                    status,
                    endpoint,
                    protocol_code,
                ))
            }
            _ => AdminWriteFailure::OutcomeUnknown {
                code: format!("http_{status}"),
                response_status: Some(status),
            },
        }
    }

    /// 沿用页面测试已有的领域请求夹具，保留实际 API 标记的端点类别。
    #[cfg(test)]
    pub(super) fn decode_stub_failure(self, status: u16, body: &str) -> AdminWriteFailure {
        Self::write_failure(self.endpoint, DecodedHttpFailure::from_bytes(status, body.as_bytes()))
    }
}

/// 为非命令请求指定等待预算，沿用其既有稳定错误码。
pub(crate) async fn send_request_with_timeout(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
    timeout_millis: u32,
) -> Result<AdminResponse, String> {
    send_request_once(build, timeout_millis).await.map_err(|failure| match failure {
        AdminWriteFailure::NotSent { code } => AdminApiFailure::not_sent_code(&code),
        AdminWriteFailure::OutcomeUnknown { code, .. } => code,
        failure @ AdminWriteFailure::VerifiedHttp(_) => failure.display_code().into_owned(),
    })
}

async fn send_request_once(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
    timeout_millis: u32,
) -> Result<AdminResponse, AdminWriteFailure> {
    dispatch_request(build, timeout_millis).await.map_err(RequestFailure::into_write_failure)
}

struct RequestFailure {
    built: bool,
    error: TransportError,
    build_failure: Option<String>,
}

impl RequestFailure {
    fn into_write_failure(self) -> AdminWriteFailure {
        if self.built {
            // 即使传输层已收到响应头，正文中断也不向页面暴露写响应状态。
            return AdminWriteFailure::outcome_unknown(self.error.code());
        }
        AdminWriteFailure::not_sent(
            self.build_failure.unwrap_or_else(|| self.error.code().to_owned()),
        )
    }
}

async fn dispatch_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
    timeout_millis: u32,
) -> Result<AdminResponse, RequestFailure> {
    let mut build_failure = None;
    let mut built = false;
    let mut invalidation = None;
    let result = auction_web_shared::send_request_with_timeout(
        |abort_signal| match build(abort_signal) {
            Ok(request) => {
                built = true;
                invalidation = AdminSessionInvalidation::capture();
                Ok(request)
            }
            Err(code) => {
                build_failure = Some(code);
                Err(TransportError::Network(None))
            }
        },
        timeout_millis,
    )
    .await;
    result
        .map(|response| AdminResponse::new(response, invalidation))
        .map_err(|error| RequestFailure { built, error, build_failure })
}

fn transport_error_code(error: TransportError) -> String {
    error.code().to_owned()
}

/// 写请求的 CSRF 门禁：取得 token 前后都重读写许可，并核对会话身份未被其他标签页替换。
pub(crate) async fn fetch_csrf_token() -> Result<String, String> {
    check_csrf_token().await.map_err(|code| AdminApiFailure::not_sent_code(&code))
}

async fn review_write_session() -> Result<String, String> {
    #[cfg(test)]
    if let Some(stub) = try_consume_context::<WriteTransportStub>() {
        return (stub.review)();
    }
    check_csrf_token().await
}

async fn check_csrf_token() -> Result<String, String> {
    // 认证内状态不等于继续授权；会话复核未确认时不发起新的管理写入。
    if !admin_session_writes_allowed() {
        return Err("unauthenticated".to_owned());
    }
    let Some(expected) = try_consume_context::<AdminSession>() else {
        return Err("unauthenticated".to_owned());
    };
    let review = try_consume_context::<AdminSessionWriteReview>();
    if review.is_some_and(|review| !review.is_current()) {
        return Err("unauthenticated".to_owned());
    }
    let result = fetch_admin_session().await;
    if review.is_some_and(|review| !review.is_current()) {
        return Err("unauthenticated".to_owned());
    }
    let current = match result {
        Ok(Some(current)) => current,
        Ok(None) => return Err("unauthenticated".to_owned()),
        Err(code) => {
            // 无可信失效证据的复核故障沿用普通重查；可信 401 已随响应归属交给外壳。
            request_admin_session_check();
            // 此处只有会话 GET 被发送；写入未开始，不能把 GET 的超时或坏响应当作写入结果未知。
            return Err(code);
        }
    };
    // 其他标签页可能已重新登录；新会话的 token 不能授权旧宿主保存的命令或草稿。
    // 同宿主的写许可即使已被暂停，有效的异代正文仍要先交给宿主撤销 Key 展示。
    if !expected.same_identity(&current) {
        if let Some(review) = review {
            review.observe.call((review.epoch, current));
        } else {
            request_admin_session_check();
        }
        return Err("unauthenticated".to_owned());
    }
    if !admin_session_writes_allowed()
        || !try_consume_context::<AdminSession>()
            .is_some_and(|confirmed| confirmed.same_identity(&expected))
    {
        return Err("unauthenticated".to_owned());
    }
    Ok(current.csrf_token)
}

/// 共享传输层的原始派发结果。收到响应头后仍可能读不到完整正文。
#[cfg(test)]
pub(crate) enum WriteDispatchStub {
    Failed { built: bool, error: TransportError },
    Response { status: u16, body: Option<&'static [u8]> },
    OwnedResponse { status: u16, body: Vec<u8> },
}

#[cfg(test)]
impl WriteDispatchStub {
    fn into_response(self) -> Result<WriteResponse, RequestFailure> {
        let (built, error) = match self {
            Self::Failed { built, error } => (built, error),
            Self::Response { status, body: Some(bytes) } => {
                return Ok(WriteResponse::Stub { status, bytes: Cow::Borrowed(bytes) });
            }
            Self::OwnedResponse { status, body } => {
                return Ok(WriteResponse::Stub { status, bytes: Cow::Owned(body) });
            }
            // 共享传输层在同一预算内读完正文，断流会带着已收到的状态返回传输错误。
            Self::Response { status, body: None } => (true, TransportError::Network(Some(status))),
        };
        Err(RequestFailure { built, error, build_failure: None })
    }
}

/// 供共用样本从真实发送器取得回执，调用方在当前作用域安装 I/O 替身。
#[cfg(test)]
pub(crate) async fn send_stub_write(
    url: &str,
    method: Method,
    endpoint: AdminWriteEndpoint,
    body: &str,
) -> Result<AdminResponse, AdminWriteFailure> {
    WriteRequest::new(url, method, endpoint).send_body(body).await
}

#[cfg(test)]
type StubWriteDispatch = dyn Fn(&str, &Method, &[u8], u32) -> WriteDispatchStub;

/// 宿主测试只替换会话复核、写请求派发和响应正文这三处 I/O。
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct WriteTransportStub {
    review: std::rc::Rc<dyn Fn() -> Result<String, String>>,
    dispatch: std::rc::Rc<StubWriteDispatch>,
}

#[cfg(test)]
impl WriteTransportStub {
    /// 为当前组件作用域安装写入 I/O 替身。
    pub(crate) fn install(
        review: impl Fn() -> Result<String, String> + 'static,
        dispatch: impl Fn(&str, &Method, &[u8], u32) -> WriteDispatchStub + 'static,
    ) {
        dioxus::prelude::provide_context(Self {
            review: std::rc::Rc::new(review),
            dispatch: std::rc::Rc::new(dispatch),
        });
    }
}

#[cfg(test)]
impl AdminWriteFailure {
    /// 解码替身提供的原始写回执，沿用生产发送器的裁定。
    pub(crate) fn from_test_http(
        endpoint: AdminWriteEndpoint,
        status: u16,
        body: impl AsRef<[u8]>,
    ) -> Self {
        WriteRequest::write_failure(endpoint, DecodedHttpFailure::from_bytes(status, body.as_ref()))
    }
}

#[cfg(test)]
impl AdminApiFailure {
    /// 宿主测试在传输边界提供原始 HTTP 回执，端点仍由实际 API 函数标记。
    pub(crate) fn stub_write_response(
        url: &'static str,
        method: Method,
        status: u16,
        body: &'static str,
    ) {
        WriteTransportStub::install(
            || Ok("csrf-fixture".to_owned()),
            move |actual_url, actual_method, _body, _timeout_millis| {
                assert_eq!(actual_url, url);
                assert_eq!(actual_method, &method);
                WriteDispatchStub::Response { status, body: Some(body.as_bytes()) }
            },
        );
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use dioxus::prelude::{ScopeId, VNode, VirtualDom};
    use gloo_net::http::Method;

    use super::{WriteRequest, send_request, send_request_once};
    use crate::api::{AdminApiFailure, AdminWriteEndpoint, AdminWriteFailure};

    #[test]
    fn failed_request_construction_stops_before_fetch() {
        let mut future = pin!(send_request(|_| Err("network_error".to_owned())));
        let Poll::Ready(Err(code)) = future.as_mut().poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("请求构造失败应在发送前返回");
        };
        assert!(AdminApiFailure::definite_rejection(&code));
        assert_eq!(AdminApiFailure::display_code(&code), "network_error");
        assert!(!AdminApiFailure::definite_rejection("network_error"));
    }

    #[test]
    fn write_request_construction_failure_is_not_sent() {
        let mut future = pin!(send_request_once(|_| Err("invalid_request".to_owned()), 100));
        let Poll::Ready(Err(failure)) =
            future.as_mut().poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("请求构造失败应在发送前返回");
        };
        assert_eq!(failure, AdminWriteFailure::not_sent("invalid_request"));
        assert_eq!(failure.observed_response_status(), None);
    }

    #[test]
    fn missing_session_stops_write_before_dispatch() {
        let dom = VirtualDom::prebuilt(VNode::empty);
        dom.in_scope(ScopeId::APP, || {
            let request = WriteRequest::general("/api/v1/admin/users", Method::POST);
            let mut future = pin!(request.send_body("{}"));
            let Poll::Ready(Err(failure)) =
                future.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            else {
                panic!("未登录时写请求应直接返回未发送");
            };
            assert_eq!(failure, AdminWriteFailure::not_sent("unauthenticated"));
            assert_eq!(failure.observed_response_status(), None);
        });
    }

    #[test]
    fn sender_verifies_only_complete_and_unmarked_write_envelopes() {
        let dom = VirtualDom::prebuilt(VNode::empty);
        for (body, verified) in [
            (
                r#"{"code":"state_conflict","message":"版本已变化","data":null,"request_id":null}"#,
                true,
            ),
            (r#"{"code":"state_conflict","message":"缺少 data","request_id":null}"#, false),
            (
                r#"{"code":"write_endpoint:user-mutation:409:state_conflict","message":"伪端点","data":null,"request_id":null}"#,
                false,
            ),
        ] {
            dom.in_scope(ScopeId::APP, || {
                let url = "/api/v1/admin/users";
                AdminApiFailure::stub_write_response(url, Method::PATCH, 409, body);
                let request =
                    WriteRequest::new(url, Method::PATCH, AdminWriteEndpoint::UserMutation);
                let mut future = pin!(
                    request.send_body(r#"{"command_id":"00000000-0000-4000-8000-000000000001"}"#)
                );
                let Poll::Ready(Err(failure)) =
                    future.as_mut().poll(&mut Context::from_waker(Waker::noop()))
                else {
                    panic!("写失败应返回原始回执的裁定");
                };
                assert_eq!(failure.observed_response_status(), Some(409));
                assert_eq!(failure.is_verified_rejection("state_conflict"), verified);
                if verified {
                    let AdminWriteFailure::VerifiedHttp(receipt) = failure else {
                        panic!("完整写回执应保留证据");
                    };
                    assert_eq!(receipt.endpoint(), AdminWriteEndpoint::UserMutation);
                    assert_eq!(receipt.protocol_code(), "state_conflict");
                } else {
                    assert!(matches!(failure, AdminWriteFailure::OutcomeUnknown { .. }), "{body}");
                    assert_eq!(failure.display_code(), "http_409", "{body}");
                }
            });
        }
    }
}
