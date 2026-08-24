//! Public API 的服务端时间校准与同源请求边界。

use chrono::{DateTime, Utc};
use gloo_net::http::{Request, Response};
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

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
        let sample = server_date
            .timestamp_millis()
            .saturating_add(SERVER_DATE_CENTER_MILLIS)
            .saturating_sub(browser_now as i64);
        if !SERVER_OFFSET_KNOWN.swap(true, Ordering::AcqRel) {
            SERVER_OFFSET_MILLIS.store(sample, Ordering::Release);
            return;
        }
        let current = SERVER_OFFSET_MILLIS.load(Ordering::Acquire);
        let adjusted = current.saturating_add(
            sample.saturating_sub(current).saturating_mul(SERVER_OFFSET_EMA_NUMERATOR)
                / SERVER_OFFSET_EMA_DENOMINATOR,
        );
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
        {
            let corrected =
                (browser_now as i64).saturating_add(SERVER_OFFSET_MILLIS.load(Ordering::Acquire));
            return DateTime::from_timestamp_millis(corrected).unwrap_or_else(Utc::now);
        }
    }
    Utc::now()
}

/// 发送 GET 并在读取 DTO 前采集服务端时钟；所有 Public 读取共用该边界。
pub(crate) async fn send_get(url: &str) -> Result<Response, String> {
    let response = Request::get(url).send().await.map_err(|error| error.to_string())?;
    observe_response_date(&response);
    Ok(response)
}
