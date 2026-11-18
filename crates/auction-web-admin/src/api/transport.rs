//! Admin 同源请求发送与写前 CSRF 复核。
//! 传输边界复用 `auction-web-shared` 的统一超时与错误分类；本层只把结构化错误折回稳定 code。
//! `x-csrf-token` 与 Basic Auth header 由调用方在 `build` 闭包叠加（共享层不感知凭据）。

use auction_web_shared::TransportError;
use dioxus::prelude::try_consume_context;
use gloo_net::http::{Method, Request, RequestBuilder, Response};

use super::failure::{AdminApiFailure, invalid_request_error, response_failure};
use super::session::{
    AdminSession, admin_session_writes_allowed, fetch_admin_session, request_admin_session_check,
};

/// 发送不改写 URL 的 Admin GET 请求。
pub(crate) async fn send_get(url: &str) -> Result<Response, String> {
    auction_web_shared::send_get(url).await.map_err(transport_error_code)
}

/// 在共享传输层的统一超时上限内发送一次 Admin 请求。
/// 保存构建阶段：只有成功构造请求、交给 fetch 后的故障可能对应已提交的业务写入。
pub(crate) async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
) -> Result<Response, String> {
    send_request_with_timeout(build, auction_web_shared::DEFAULT_REQUEST_TIMEOUT_MILLIS).await
}

/// 发送一次带 CSRF 复核的 JSON 写请求，保留错误回执以便调用方按端点分类。
pub(super) async fn send_json_write(
    url: &str,
    method: Method,
    command: &impl serde::Serialize,
) -> Result<Response, AdminApiFailure> {
    #[cfg(test)]
    if let Some(receipt) = try_consume_context::<WriteResponseStub>() {
        assert_eq!(url, receipt.url);
        assert_eq!(method, receipt.method);
        return Err(AdminApiFailure::from_http_body(receipt.status, receipt.body));
    }
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let response = send_request(|abort_signal| {
        RequestBuilder::new(url)
            .method(method)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if response.ok() { Ok(response) } else { Err(response_failure(response).await) }
}

/// 为单次 Admin 请求指定等待预算；仍复用共享 fetch，不改变其他请求预算或添加重试。
pub(crate) async fn send_request_with_timeout(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
    timeout_millis: u32,
) -> Result<Response, String> {
    let mut build_failure = None;
    let mut built = false;
    let result = auction_web_shared::send_request_with_timeout(
        |abort_signal| match build(abort_signal) {
            Ok(request) => {
                built = true;
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
    match result {
        Ok(response) => Ok(response),
        Err(error) if built => Err(transport_error_code(error)),
        Err(error) => Err(AdminApiFailure::not_sent_code(
            build_failure.as_deref().unwrap_or_else(|| error.code()),
        )),
    }
}

fn transport_error_code(error: TransportError) -> String {
    error.code().to_owned()
}

/// 写请求的 CSRF 门禁：取得 token 前后都重读写许可，并核对会话身份未被其他标签页替换。
pub(crate) async fn fetch_csrf_token() -> Result<String, String> {
    // 认证内状态不等于继续授权；会话复核未确认时不发起新的管理写入。
    if !admin_session_writes_allowed() {
        return Err("unauthenticated".to_owned());
    }
    let current = match fetch_admin_session().await {
        Ok(Some(current)) => current,
        result => {
            // CSRF 复核失败也撤回写许可；旧宿主与未决命令交给既有会话重查流程保留。
            request_admin_session_check();
            let code = result.err().unwrap_or_else(|| "unauthenticated".to_owned());
            // 此处只有会话 GET 被发送；写入未开始，不能把 GET 的超时或坏响应当作写入结果未知。
            return Err(AdminApiFailure::not_sent_code(&code));
        }
    };
    // 获取 CSRF 的等待期间也可能进入暂时未确认状态，不能带着旧页面授权继续提交。
    if !admin_session_writes_allowed() {
        return Err("unauthenticated".to_owned());
    }
    // 其他标签页可能已重新登录；新会话的 token 不能授权旧宿主保存的命令或草稿。
    let same_identity = try_consume_context::<AdminSession>().is_some_and(|confirmed| {
        confirmed.actor == current.actor && confirmed.csrf_token == current.csrf_token
    });
    if !same_identity {
        request_admin_session_check();
        return Err("unauthenticated".to_owned());
    }
    Ok(current.csrf_token)
}

#[cfg(test)]
#[derive(Clone)]
struct WriteResponseStub {
    url: &'static str,
    method: Method,
    status: u16,
    body: &'static str,
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
        dioxus::prelude::provide_context(WriteResponseStub { url, method, status, body });
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    use super::send_request;
    use crate::api::AdminApiFailure;

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
}
