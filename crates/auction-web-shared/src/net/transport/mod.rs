//! 同源请求的共享传输边界。
//! 唯一的 fetch 实现：默认 10 秒、可按单次请求指定的取消预算与结构化 [`TransportError`] 分类。
//! Admin 的 CSRF token 与 Basic Auth **不进入本层**：调用方在 `build` 闭包里建立
//! `web_sys::Request` 时自行叠加 header，本层只负责发送、超时取消与错误分类。

mod browser_lock;

pub use browser_lock::{BrowserLock, BrowserLockError};

#[cfg(target_arch = "wasm32")]
use futures_util::{
    future::{Either, select},
    pin_mut,
};
use gloo_net::http::{Request, Response};
#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;

/// 两端共用的默认请求超时上限（恢复旧版 10 秒语义）。
pub const DEFAULT_REQUEST_TIMEOUT_MILLIS: u32 = 10_000;

/// 共享传输层的结构化错误。
/// 不向组件传播底层错误正文；surface 用 [`TransportError::code`] 取稳定 transport code，
/// 再经错误码映射层（见 `errors` 模块）转成本地化文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// 响应头与正文未在本次预算内完整读取；默认 [`DEFAULT_REQUEST_TIMEOUT_MILLIS`]，fetch 已取消。
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

/// 发送一次 GET 并保留调用方给出的精确 URL（依赖升级不得改变分页/筛选请求的构造）。
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

/// 在共享超时上限内完整读取响应头与正文；超时或调用方任务取消时终止 fetch。
/// `build` 在 WASM 下收到本次请求的 `AbortSignal`（非 WASM 为 `None`），用于叠加 surface
/// 特有的 header 与请求体。非 WASM 构建无浏览器 fetch，直接发送且不启用超时（仅静态检查）。
pub async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, TransportError>,
) -> Result<Response, TransportError> {
    send_request_with_timeout(build, DEFAULT_REQUEST_TIMEOUT_MILLIS).await
}

/// 为单次请求显式指定预算；复用相同的正文读取、取消和错误边界，不添加自动重试。
pub async fn send_request_with_timeout(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, TransportError>,
    timeout_millis: u32,
) -> Result<Response, TransportError> {
    send_request_observed_with_timeout(build, |_| {}, timeout_millis).await
}

/// 允许 surface 在响应头到达时校时，避免把正文传输耗时误算为时钟偏差；返回前仍完整读取正文。
pub async fn send_request_observed(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, TransportError>,
    observe: impl FnOnce(&Response),
) -> Result<Response, TransportError> {
    send_request_observed_with_timeout(build, observe, DEFAULT_REQUEST_TIMEOUT_MILLIS).await
}

async fn send_request_observed_with_timeout(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, TransportError>,
    observe: impl FnOnce(&Response),
    timeout_millis: u32,
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
            observe_response_build(&response);
            observe(&response);
            let status = response.status();
            let mut body =
                response.binary().await.map_err(|_| TransportError::Network(Some(status)))?;
            // 保留所有调用方消费的状态与响应头；正文已读完，后续 JSON 解码不再等网络。
            // 204/205/304 按 Fetch 约定必须使用 null body。
            let builder = Response::builder()
                .status(status)
                .status_text(&response.status_text())
                .headers(response.headers());
            let body =
                if matches!(status, 204 | 205 | 304) { None } else { Some(body.as_mut_slice()) };
            builder.body(body).map_err(|_| TransportError::Decode)
        };
        let timeout = TimeoutFuture::new(timeout_millis);
        pin_mut!(send, timeout);
        return match select(send, timeout).await {
            Either::Left((response, _)) => response,
            Either::Right((_, send)) => {
                controller.0.abort();
                // 取消后消费 fetch/body 的终止结果，再允许外层跨标签页锁释放。
                let _ = send.await;
                Err(TransportError::Timeout)
            }
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = timeout_millis;
        let response = build(None)?.send().await.map_err(|_| TransportError::Network(None))?;
        observe_response_build(&response);
        observe(&response);
        Ok(response)
    }
}

/// 每个成功收到的响应都比对一次服务端构建号；头缺失时保持原状，由实时首帧兜底。
/// `gloo_net::http::Headers::get` 在两个目标都返回 `Option<String>`。
fn observe_response_build(response: &Response) {
    if let Some(build) = response.headers().get(crate::SERVER_BUILD_HEADER) {
        crate::observe_server_build_id(&build);
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
