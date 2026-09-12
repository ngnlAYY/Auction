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
    /// 响应头与正文未在 [`DEFAULT_REQUEST_TIMEOUT_MILLIS`] 内完整读取；fetch 已被取消，
    /// 避免迟到响应改写页面。
    Timeout,
    /// 浏览器层网络失败或无法建立请求；正文读取失败保留已收到的 HTTP 状态。
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
    send_get_observed(url, |_| {}).await
}

/// 在响应头到达时观察元数据；正文仍须在同一超时预算内完整读取。
pub async fn send_get_observed(
    url: &str,
    observe: impl FnOnce(&Response),
) -> Result<Response, TransportError> {
    send_request_observed(
        |abort_signal| {
            let options = web_sys::RequestInit::new();
            options.set_method("GET");
            options.set_signal(abort_signal);
            web_sys::Request::new_with_str_and_init(url, &options)
                .map(Request::from)
                .map_err(|_| TransportError::Network(None))
        },
        observe,
    )
    .await
}

/// 在共享超时上限内完整读取响应头与正文；返回的响应只从内存读取正文。
/// 超时或调用方任务取消时终止 fetch，避免遗留网络请求。
///
/// `build` 在 WASM 下收到本次请求的 `AbortSignal`（非 WASM 构建为 `None`），调用方
/// 在闭包内叠加 surface 特有的 header（如 Admin 的 CSRF/Basic Auth）与请求体。非
/// WASM 构建不存在浏览器 fetch，直接发送且不启用超时边界（与两端原实现一致，仅服务
/// 于静态检查）。
pub async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, TransportError>,
) -> Result<Response, TransportError> {
    send_request_observed(build, |_| {}).await
}

/// 允许 surface 在响应头到达时校时，避免把正文传输耗时误算为时钟偏差。
/// 返回前仍完整读取正文；观察回调不得承担正文读取。
pub async fn send_request_observed(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, TransportError>,
    observe: impl FnOnce(&Response),
) -> Result<Response, TransportError> {
    #[cfg(target_arch = "wasm32")]
    {
        let controller = AbortOnDrop(
            web_sys::AbortController::new().map_err(|_| TransportError::Network(None))?,
        );
        let signal = controller.0.signal();
        let request = build(Some(&signal))?;
        let send = async move {
            let response = request.send().await.map_err(|_| TransportError::Network(None))?;
            observe(&response);
            let status = response.status();
            let mut body =
                response.binary().await.map_err(|_| TransportError::Network(Some(status)))?;
            // 保留所有调用方消费的状态与响应头；正文已完成网络读取，后续 JSON 解码
            // 不会重新等待网络。204/205/304 按 Fetch 约定必须使用 null body。
            let builder = Response::builder()
                .status(status)
                .status_text(&response.status_text())
                .headers(response.headers());
            let body =
                if matches!(status, 204 | 205 | 304) { None } else { Some(body.as_mut_slice()) };
            builder.body(body).map_err(|_| TransportError::Decode)
        };
        let timeout = TimeoutFuture::new(DEFAULT_REQUEST_TIMEOUT_MILLIS);
        pin_mut!(send, timeout);
        return match select(send, timeout).await {
            Either::Left((response, _)) => response,
            Either::Right((_, _)) => {
                controller.0.abort();
                Err(TransportError::Timeout)
            }
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = DEFAULT_REQUEST_TIMEOUT_MILLIS;
        let response = build(None)?.send().await.map_err(|_| TransportError::Network(None))?;
        observe(&response);
        Ok(response)
    }
}

/// 资源重启或组件卸载会丢弃 future；此时也须取消仍在读取的浏览器请求。
#[cfg(target_arch = "wasm32")]
struct AbortOnDrop(web_sys::AbortController);

#[cfg(target_arch = "wasm32")]
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
