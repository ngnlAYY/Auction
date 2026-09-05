//! 同源请求的共享传输边界。
//!
//! 两端原先各自维护一份逐行相同的 fetch 封装（Public 在 `api/clock.rs`、Admin 在
//! `api/mod.rs`）：10 秒 `AbortController` 取消边界、gloo-net 0.6.0 空 QueryParams
//! workaround，以及用 `"请求超时"` 中文字符串比较分类超时。本模块把这三件事收敛为
//! 唯一实现，错误改用结构化 [`TransportError`]，字符串分类彻底移除。
//!
//! Admin 特有的 CSRF token 与 Basic Auth **不进入本层**：调用方在 `build` 闭包建立
//! `web_sys::Request` 时自行叠加所需 header（surface 层职责），本层只负责发送、
//! 超时取消与错误分类，不知道任何凭据的存在。

#[cfg(target_arch = "wasm32")]
use futures_util::{
    future::{Either, select},
    pin_mut,
};
use gloo_net::http::{Request, Response};
#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;

/// 两端共用的请求超时上限（恢复旧版 10 秒语义）。
pub const DEFAULT_REQUEST_TIMEOUT_MILLIS: u32 = 10_000;

/// 共享传输层的结构化错误。
///
/// 不向组件传播底层错误正文；surface 用 [`TransportError::code`] 取得契约中的稳定
/// transport code，再经错误码映射层（见 `errors` 模块）转成本地化文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// 请求在 [`DEFAULT_REQUEST_TIMEOUT_MILLIS`] 内未完成；浏览器 fetch 已被取消，
    /// 避免迟到响应改写页面。
    Timeout,
    /// 浏览器层网络失败或无法建立请求。`Some(status)` 保留给调用方附加上下文；
    /// 本层自身产生的网络失败不携带 HTTP 状态（恒为 `None`）。
    Network(Option<u16>),
    /// 响应正文无法按预期 DTO 解码。
    Decode,
}

impl TransportError {
    /// 映射为两端契约中的稳定 transport code。
    pub const fn code(self) -> &'static str {
        match self {
            Self::Timeout => "request_timeout",
            Self::Network(_) => "network_error",
            Self::Decode => "invalid_response",
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for TransportError {}

/// 发送一次 GET 并保留调用方给出的精确 URL。
///
/// gloo-net 0.6.0 会把已有 query 与空 QueryParams 拼成 `...?a=b&`；旧客户端使用精确
/// fetch URL，因此在共用 GET 边界直接建立浏览器请求，避免所有分页/筛选 URL 被改写。
pub async fn send_get(url: &str) -> Result<Response, TransportError> {
    send_request(|abort_signal| {
        let options = web_sys::RequestInit::new();
        options.set_method("GET");
        options.set_signal(abort_signal);
        web_sys::Request::new_with_str_and_init(url, &options)
            .map(Request::from)
            .map_err(|_| TransportError::Network(None))
    })
    .await
}

/// 在共享超时上限内发送一次请求；超时后取消浏览器 fetch，避免迟到响应改写页面。
///
/// `build` 在 WASM 下收到本次请求的 `AbortSignal`（非 WASM 构建为 `None`），调用方
/// 在闭包内叠加 surface 特有的 header（如 Admin 的 CSRF/Basic Auth）与请求体。非
/// WASM 构建不存在浏览器 fetch，直接发送且不启用超时边界（与两端原实现一致，仅服务
/// 于静态检查）。
pub async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, TransportError>,
) -> Result<Response, TransportError> {
    #[cfg(target_arch = "wasm32")]
    {
        let controller =
            web_sys::AbortController::new().map_err(|_| TransportError::Network(None))?;
        let signal = controller.signal();
        let request = build(Some(&signal))?;
        let send = request.send();
        let timeout = TimeoutFuture::new(DEFAULT_REQUEST_TIMEOUT_MILLIS);
        pin_mut!(send, timeout);
        return match select(send, timeout).await {
            Either::Left((response, _)) => response.map_err(|_| TransportError::Network(None)),
            Either::Right((_, _)) => {
                controller.abort();
                Err(TransportError::Timeout)
            }
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = DEFAULT_REQUEST_TIMEOUT_MILLIS;
        build(None)?.send().await.map_err(|_| TransportError::Network(None))
    }
}
