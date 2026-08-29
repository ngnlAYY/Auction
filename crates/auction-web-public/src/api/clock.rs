//! Public API 的服务端时间校准与同源请求边界。

use chrono::{DateTime, Utc};
#[cfg(target_arch = "wasm32")]
use futures_util::{
    future::{Either, select},
    pin_mut,
};
use gloo_net::http::{Request, Response};
#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

const PUBLIC_REQUEST_TIMEOUT_MILLIS: u32 = 10_000;

// 旧版以响应 Date 头估算服务端时钟，避免客户端本地时钟偏移导致竞价窗口显示错误。
#[cfg(target_arch = "wasm32")]
const SERVER_DATE_CENTER_MILLIS: i64 = 500;
#[cfg(target_arch = "wasm32")]
const SERVER_OFFSET_EMA_NUMERATOR: i64 = 3;
#[cfg(target_arch = "wasm32")]
const SERVER_OFFSET_EMA_DENOMINATOR: i64 = 10;
#[cfg(target_arch = "wasm32")]
static SERVER_OFFSET_MILLIS: AtomicI64 = AtomicI64::new(0);
#[cfg(target_arch = "wasm32")]
static SERVER_OFFSET_KNOWN: AtomicBool = AtomicBool::new(false);

/// 记录响应中的服务端时间样本；无效或缺失的 Date 头不会影响当前请求。
pub(crate) fn observe_response_date(_response: &Response) {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(raw_date) = _response.headers().get("Date") else {
            return;
        };
        let Ok(server_date) = DateTime::parse_from_rfc2822(&raw_date) else {
            return;
        };
        let browser_now = js_sys::Date::new_0().get_time();
        if !browser_now.is_finite()
            || browser_now < i64::MIN as f64
            || browser_now > i64::MAX as f64
        {
            return;
        }
        let Some(sample) = server_date
            .timestamp_millis()
            .checked_add(SERVER_DATE_CENTER_MILLIS)
            .and_then(|value| value.checked_sub(browser_now as i64))
        else {
            return;
        };
        if !SERVER_OFFSET_KNOWN.swap(true, Ordering::AcqRel) {
            SERVER_OFFSET_MILLIS.store(sample, Ordering::Release);
            return;
        }
        let current = SERVER_OFFSET_MILLIS.load(Ordering::Acquire);
        let Some(adjusted) = sample
            .checked_sub(current)
            .and_then(|delta| delta.checked_mul(SERVER_OFFSET_EMA_NUMERATOR))
            .and_then(|delta| delta.checked_div(SERVER_OFFSET_EMA_DENOMINATOR))
            .and_then(|delta| current.checked_add(delta))
        else {
            return;
        };
        SERVER_OFFSET_MILLIS.store(adjusted, Ordering::Release);
    }
}

/// 返回按服务端时钟修正后的当前时间；非 WASM 构建沿用系统 UTC 时钟。
pub(crate) fn estimated_now() -> DateTime<Utc> {
    #[cfg(target_arch = "wasm32")]
    {
        let browser_now = js_sys::Date::new_0().get_time();
        if browser_now.is_finite()
            && browser_now >= i64::MIN as f64
            && browser_now <= i64::MAX as f64
            && let Some(corrected) =
                (browser_now as i64).checked_add(SERVER_OFFSET_MILLIS.load(Ordering::Acquire))
        {
            return DateTime::from_timestamp_millis(corrected).unwrap_or_else(Utc::now);
        }
    }
    Utc::now()
}

/// 发送 GET 并在读取 DTO 前采集服务端时钟；所有 Public 读取共用该边界。
pub(crate) async fn send_get(url: &str) -> Result<Response, String> {
    let response = send_request(|abort_signal| {
        // gloo-net 0.6.0 会把已有 query 与空 QueryParams 拼成 `...?a=b&`；旧客户端使用精确
        // fetch URL，因此在共用 GET owner 直接建立浏览器请求，避免所有分页/筛选 URL 被改写。
        let options = web_sys::RequestInit::new();
        options.set_method("GET");
        options.set_signal(abort_signal);
        web_sys::Request::new_with_str_and_init(url, &options)
            .map(Request::from)
            .map_err(|_| "无法建立 GET 请求".to_owned())
    })
    .await?;
    observe_response_date(&response);
    Ok(response)
}

/// 在旧版 10 秒上限内发送一次 Public 请求；超时会取消浏览器 fetch，避免迟到响应改写页面。
pub(crate) async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, String>,
) -> Result<Response, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let controller =
            web_sys::AbortController::new().map_err(|_| "无法建立请求取消边界".to_owned())?;
        let signal = controller.signal();
        let request = build(Some(&signal))?;
        let send = request.send();
        let timeout = TimeoutFuture::new(PUBLIC_REQUEST_TIMEOUT_MILLIS);
        pin_mut!(send, timeout);
        return match select(send, timeout).await {
            Either::Left((response, _)) => response.map_err(|error| error.to_string()),
            Either::Right((_, _)) => {
                controller.abort();
                Err("请求超时".to_owned())
            }
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = PUBLIC_REQUEST_TIMEOUT_MILLIS;
        build(None)?.send().await.map_err(|error| error.to_string())
    }
}

/// 把同源请求失败收敛为旧 Public client 的稳定 transport code，不向组件传播底层错误正文。
pub(crate) fn transport_error_code(error: &str) -> String {
    if error == "请求超时" { "request_timeout" } else { "network_error" }.to_owned()
}
