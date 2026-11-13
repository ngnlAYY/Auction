//! Public API 的服务端时间校准与同源请求边界。
//! 发送、超时取消与错误分类统一由 `auction-web-shared` 的传输层承担（10 秒取消上限、
//! 精确 URL、结构化 [`TransportError`]）；本模块只保留 Public 特有的服务端时钟采样，
//! 字符串形式的 "请求超时" 比较已彻底移除。

use chrono::{DateTime, Utc};
use gloo_net::http::{Request, Response};
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use auction_web_shared::TransportError;

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

/// 响应的 `Date` 能否代表此刻的服务端时钟：只有不可能是缓存重放的响应可以。
///
/// 可缓存的响应会连同生成时的 `Date` 一起被重放，历史类读取最长保存 5 分钟、已结算内容保存
/// 一天；把它当作当前时间会让时钟偏移整体后退，倒计时与开拍刷新随之变慢，首个样本还会被
/// 直接采用。`no-store` 的响应任何缓存都不得保存（与构建号采信同一条规则）。`private` 的
/// 响应共享缓存不得保存，但浏览器自己可以重放，因此只在请求已绕过浏览器 HTTP 缓存时采信。
fn response_date_is_current(cache_control: Option<&str>, bypassed_browser_cache: bool) -> bool {
    auction_web_shared::response_reports_current_build(cache_control)
        || (bypassed_browser_cache
            && cache_control.is_some_and(|value| {
                value.split(',').any(|directive| directive.trim().eq_ignore_ascii_case("private"))
            }))
}

/// 记录响应中的服务端时间样本；无效或缺失的 Date 头不会影响当前请求。
/// 用于可能由浏览器 HTTP 缓存直接应答的请求，只采信 `no-store` 响应。
pub(crate) fn observe_response_date(response: &Response) {
    record_response_date(response, false);
}

/// 再验证请求（脚本自带 `If-None-Match` 或 `no-cache` 模式）一定到达服务端，
/// 进行中场次的 `private` 目录响应在这里同样是当前样本。
fn observe_revalidated_date(response: &Response) {
    record_response_date(response, true);
}

fn record_response_date(response: &Response, bypassed_browser_cache: bool) {
    let headers = response.headers();
    if response_date_is_current(headers.get("cache-control").as_deref(), bypassed_browser_cache) {
        record_server_date(headers.get("Date"));
    }
}

/// 把一个当前的 `Date` 样本并入时钟偏移：首个样本直接采用，其后按指数滑动平均收敛。
fn record_server_date(_raw_date: Option<String>) {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(raw_date) = _raw_date else {
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
/// 传输实现复用 `auction-web-shared` 的唯一边界；结构化错误直接向调用方传播，由调用方
/// 用 [`TransportError::code`] 折回页面消费的稳定 transport code。
pub(crate) async fn send_get(url: &str) -> Result<Response, TransportError> {
    auction_web_shared::send_get_observed(url, observe_response_date).await
}

/// 发送 SWR 缓存层的公开 GET：有 ETag 时携带 `If-None-Match`，没有时要求浏览器向服务端再验证。
/// 304 与 200 一样在共享预算内读完响应头，正文与缓存判定由调用方处理。
///
/// 这两种请求都不会由浏览器 HTTP 缓存直接应答：脚本自带条件头的请求按 Fetch 规范绕过它；
/// 无 ETag 的读取发生在缓存未命中或刚被失效之后（实时事件、命令结果），此时必须拿到服务端
/// 的当前快照。公开目录响应带 `max-age=2, stale-while-revalidate=5`，默认缓存模式下 7 秒内
/// 的第二次读取会原样拿回旧正文，并被 SWR 层当作新快照保留到 TTL 结束；`no-cache` 模式
/// 仍让浏览器用自己的验证器做条件请求，未变化时只传回 304。
pub(crate) async fn send_get_conditional(
    url: &str,
    etag: Option<&str>,
) -> Result<Response, TransportError> {
    auction_web_shared::send_request_observed(
        |abort_signal| {
            let request = Request::get(url).abort_signal(abort_signal);
            let request = match etag {
                Some(etag) => request.header("If-None-Match", etag),
                None => request.cache(web_sys::RequestCache::NoCache),
            };
            request.build().map_err(|_| TransportError::Network(None))
        },
        observe_revalidated_date,
    )
    .await
}

/// 在响应头到达时校时，并在共享总预算内完整读取命令响应正文。
pub(crate) async fn send_request(
    build: impl FnOnce(Option<&web_sys::AbortSignal>) -> Result<Request, TransportError>,
) -> Result<Response, TransportError> {
    auction_web_shared::send_request_observed(build, observe_response_date).await
}

#[cfg(test)]
mod tests {
    use super::response_date_is_current;

    #[test]
    fn only_responses_that_cannot_be_replayed_calibrate_the_server_clock() {
        // 会话、身份化读取与命令响应：任何缓存都不得保存，两类请求都采信。
        for cache_control in ["no-store", "private, No-Store"] {
            assert!(response_date_is_current(Some(cache_control), false), "{cache_control}");
            assert!(response_date_is_current(Some(cache_control), true), "{cache_control}");
        }
        // 进行中场次目录：共享缓存不得保存，但浏览器可在新鲜期内重放，只在再验证请求上采信。
        let catalog = "private, max-age=2, stale-while-revalidate=5";
        assert!(response_date_is_current(Some(catalog), true));
        assert!(!response_date_is_current(Some(catalog), false));
        // 可被浏览器或 CDN 重放的响应带着生成时的 Date；再验证也可能由 CDN 的旧副本应答。
        for replayable in [
            "public, max-age=300, s-maxage=300, stale-while-revalidate=3600",
            "public, max-age=86400, s-maxage=86400, stale-while-revalidate=604800",
            "public, max-age=5",
            "no-cache",
            "max-age=30",
            "",
        ] {
            assert!(!response_date_is_current(Some(replayable), false), "{replayable}");
            assert!(!response_date_is_current(Some(replayable), true), "{replayable}");
        }
        // 没有缓存头的响应无法判断来源。
        assert!(!response_date_is_current(None, false));
        assert!(!response_date_is_current(None, true));
    }
}
