//! 公开只读 GET 的条件请求、非 2xx 响应的缓存分类与带缓存的严格解码；其余查询都经这里读写
//! 公开读缓存。

use std::future::Future;

use gloo_net::http::Response;

use super::super::clock::send_get_conditional;
use super::super::swr_cache::{self, CacheRead, FetchOutcome};
use super::super::{TransportError, decoder};
#[cfg(test)]
use crate::surface::PageReadFixture;

/// 条件 GET 的响应分类；304 只在确实发送了 `If-None-Match` 时成立。
pub(super) enum ConditionalResponse {
    NotModified,
    Success { response: Response, etag: Option<String> },
    Failure(Response),
}

pub(super) async fn conditional_get(
    url: &str,
    etag: Option<&str>,
) -> Result<ConditionalResponse, (TransportError, Option<u16>)> {
    let response = send_get_conditional(url, etag).await?;
    if response.status() == 304 {
        return Ok(if etag.is_some() {
            ConditionalResponse::NotModified
        } else {
            // 无 ETag 的请求不应收到 304；交给错误 envelope 解码归一为 invalid_response。
            ConditionalResponse::Failure(response)
        });
    }
    if !response.ok() {
        return Ok(ConditionalResponse::Failure(response));
    }
    let etag = response.headers().get("ETag");
    Ok(ConditionalResponse::Success { response, etag })
}

/// 非 2xx 响应的缓存分类：429（读闸繁忙或来源超额）、503（读端口暂不可用），以及源站不可达时
/// 网关返回的 502/504 与 CDN 的 520–524，都是暂时性拒绝，与断网同样处理：缓存层有可回退的
/// 旧快照时继续展示。404/410 已在收到响应头时驱逐旧条目，这里与其他拒绝一样返回错误。
pub(super) fn failure_outcome<T, E>(status: u16, error: E) -> FetchOutcome<T, E> {
    match status {
        429 | 502 | 503 | 504 | 520..=524 => FetchOutcome::Busy(error),
        _ => FetchOutcome::Rejected(error),
    }
}

/// 正文传输失败仍以已知的非 2xx 状态决定旧快照能否回退。
pub(super) fn transport_failure_outcome<T, E>(status: Option<u16>, error: E) -> FetchOutcome<T, E> {
    match status {
        Some(status) if !(200..300).contains(&status) => failure_outcome(status, error),
        _ => FetchOutcome::Transport(error),
    }
}

/// 对公开只读 GET 执行带缓存的严格解码。
/// `decode` 必须完成全部跨字段校验（含响应与请求身份的绑定）；只有它返回 Ok 的正文才写入
/// 缓存。传输错误与暂时性拒绝（见 [`failure_outcome`]）由缓存层决定是否回退旧值。
pub(super) async fn cached_public_get<T, D, Fut>(
    url: &str,
    ttl_millis: u64,
    decode: D,
) -> Result<T, String>
where
    T: Clone + 'static,
    D: FnOnce(Response) -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    cached_public_read(url, ttl_millis, decode).await.map(|read| read.value)
}

/// 与 [`cached_public_get`] 相同，但保留“暂时性失败后回退旧快照”的标记与确认时刻，供需要
/// 稍后再读的目录读取与须标明数据时点的页面使用。
pub(in crate::api) async fn cached_public_read<T, D, Fut>(
    url: &str,
    ttl_millis: u64,
    decode: D,
) -> Result<CacheRead<T>, String>
where
    T: Clone + 'static,
    D: FnOnce(Response) -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    #[cfg(test)]
    if let Some(result) = PageReadFixture::<String, Result<T, String>>::response(url.to_owned()) {
        return result.map(|value| CacheRead { value, stale_fallback: None });
    }
    let request_url = url.to_owned();
    swr_cache::read_or_fetch(url, ttl_millis, swr_cache::now_millis, move |etag| async move {
        match conditional_get(&request_url, etag.as_deref()).await {
            Err((error, status)) => transport_failure_outcome(status, error.code().to_owned()),
            Ok(ConditionalResponse::NotModified) => FetchOutcome::NotModified,
            Ok(ConditionalResponse::Failure(response)) => {
                let status = response.status();
                failure_outcome(status, decoder::decode_error_code(response).await)
            }
            Ok(ConditionalResponse::Success { response, etag }) => match decode(response).await {
                Ok(value) => FetchOutcome::Changed { value, etag },
                Err(error) => FetchOutcome::Rejected(error),
            },
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::future::ready;

    use futures_util::FutureExt;

    use super::{failure_outcome, transport_failure_outcome};
    use crate::api::swr_cache::{
        FetchOutcome, LIVE_TTL_MILLIS, get_or_fetch, observe_response_status,
    };

    fn assert_missing_snapshot_cannot_fall_back(
        url: &str,
        status: u16,
        missing: FetchOutcome<&'static str, &'static str>,
        error: &'static str,
    ) {
        let now = Cell::new(0);
        let calls = Cell::new(0);
        let observed_etag = Cell::new(None);
        let read = |outcome, status| {
            get_or_fetch(
                url,
                LIVE_TTL_MILLIS,
                || now.get(),
                |etag| {
                    calls.set(calls.get() + 1);
                    observed_etag.set(etag);
                    if let Some(status) = status {
                        observe_response_status(url, status);
                    }
                    ready(outcome)
                },
            )
            .now_or_never()
            .expect("替身读取应立即完成")
        };
        assert_eq!(
            read(
                FetchOutcome::Changed { value: "visible", etag: Some("etag-visible".to_owned()) },
                None
            ),
            Ok("visible")
        );
        assert_eq!(read(FetchOutcome::Transport("unused"), None), Ok("visible"));
        assert_eq!(calls.get(), 1);
        now.set(LIVE_TTL_MILLIS);
        assert_eq!(read(missing, Some(status)), Err(error));
        assert_eq!(observed_etag.take().as_deref(), Some("etag-visible"));
        assert_eq!(read(FetchOutcome::Transport("offline"), None), Err("offline"));
        assert_eq!(observed_etag.take(), None);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn not_found_and_gone_evict_a_cached_snapshot_before_the_next_read() {
        for status in [404, 410] {
            let url = format!("/api/v1/lots/lot?missing_status={status}");
            assert_missing_snapshot_cannot_fall_back(
                &url,
                status,
                failure_outcome(status, "not_found"),
                "not_found",
            );
        }
    }

    #[test]
    fn interrupted_missing_body_still_evicts_a_cached_snapshot() {
        for (status, error) in [(404, "network_error"), (410, "request_timeout")] {
            let url = format!("/api/v1/lots/lot?missing_body_status={status}");
            assert_missing_snapshot_cannot_fall_back(
                &url,
                status,
                transport_failure_outcome(Some(status), error),
                error,
            );
        }
    }

    #[test]
    fn only_busy_and_gateway_statuses_are_transient_rejections() {
        for status in [429, 502, 503, 504, 520, 521, 522, 523, 524] {
            assert!(
                matches!(failure_outcome::<(), _>(status, "busy"), FetchOutcome::Busy("busy")),
                "{status}"
            );
            assert!(matches!(
                transport_failure_outcome::<(), _>(Some(status), "interrupted"),
                FetchOutcome::Busy("interrupted")
            ));
        }
        // 其余非 2xx（含无 ETag 却收到的 304、500、CDN 的 525/526 证书错误与业务拒绝）仍失败关闭。
        for status in [304, 400, 401, 403, 404, 409, 410, 422, 500, 501, 525, 526] {
            assert!(
                matches!(
                    failure_outcome::<(), _>(status, "rejected"),
                    FetchOutcome::Rejected("rejected")
                ),
                "{status}"
            );
            assert!(matches!(
                transport_failure_outcome::<(), _>(Some(status), "interrupted"),
                FetchOutcome::Rejected("interrupted")
            ));
        }
        for status in [None, Some(200)] {
            assert!(matches!(
                transport_failure_outcome::<(), _>(status, "interrupted"),
                FetchOutcome::Transport("interrupted")
            ));
        }
    }
}
