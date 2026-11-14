//! Admin 同源请求发送与写前 CSRF 复核。
//! 传输边界复用 `auction-web-shared` 的统一超时与错误分类；本层只把结构化错误折回稳定 code。
//! `x-csrf-token` 与 Basic Auth header 由调用方在 `build` 闭包叠加（共享层不感知凭据）。

use auction_web_shared::TransportError;
use dioxus::prelude::try_consume_context;
use gloo_net::http::{Request, Response};

use super::session::{
    AdminSession, admin_session_writes_allowed, fetch_admin_session, request_admin_session_check,
};

/// 发送不改写 URL 的 Admin GET 请求。
pub(crate) async fn send_get(url: &str) -> Result<Response, String> {
    auction_web_shared::send_get(url).await.map_err(transport_error_code)
}

/// 在共享传输层的统一超时上限内发送一次 Admin 请求。
/// `build` 失败携带稳定业务 code，[`TransportError`] 无法承载，先暂存再原样返回。
pub(crate) async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
) -> Result<Response, String> {
    send_request_with_timeout(build, auction_web_shared::DEFAULT_REQUEST_TIMEOUT_MILLIS).await
}

/// 为单次 Admin 请求指定等待预算；仍复用共享 fetch，不改变其他请求预算或添加重试。
pub(crate) async fn send_request_with_timeout(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
    timeout_millis: u32,
) -> Result<Response, String> {
    let build_failure = std::cell::RefCell::new(None::<String>);
    let result = auction_web_shared::send_request_with_timeout(
        |abort_signal| {
            build(abort_signal).map_err(|code| {
                // 占位错误不会被外层使用：build 失败时以暂存的业务 code 为准。
                *build_failure.borrow_mut() = Some(code);
                TransportError::Network(None)
            })
        },
        timeout_millis,
    )
    .await;
    match result {
        Ok(response) => Ok(response),
        Err(error) => Err(build_failure.into_inner().unwrap_or_else(|| error.code().to_owned())),
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
            return Err(result.err().unwrap_or_else(|| "unauthenticated".to_owned()));
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
