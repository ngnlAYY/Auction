//! 公开只读 GET 的条件请求、非 2xx 响应的缓存分类与带缓存的严格解码；其余查询都经这里读写
//! 公开读缓存。

use std::future::Future;

use gloo_net::http::Response;

use super::super::swr_cache::{self, CacheRead, FetchOutcome};
use super::super::{TransportError, decoder, send_get_conditional};

/// 条件 GET 的响应分类；304 只在确实发送了 `If-None-Match` 时成立。
pub(super) enum ConditionalResponse {
    NotModified,
    Success { response: Response, etag: Option<String> },
    Failure(Response),
}

pub(super) async fn conditional_get(
    url: &str,
    etag: Option<&str>,
) -> Result<ConditionalResponse, TransportError> {
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
/// 旧快照时继续展示。其余状态（含无 ETag 却收到的 304、500 与业务拒绝）失败关闭。
pub(super) fn failure_outcome<T, E>(status: u16, error: E) -> FetchOutcome<T, E> {
    if matches!(status, 429 | 502 | 503 | 504 | 520..=524) {
        FetchOutcome::Busy(error)
    } else {
        FetchOutcome::Rejected(error)
    }
}

/// 对公开只读 GET 执行带缓存的严格解码。
/// `decode` 必须完成全部跨字段校验（含响应与请求身份的绑定）；只有它返回 Ok 的正文才写入
/// 缓存，4xx/5xx 或校验失败不会留下条目。传输错误与暂时性拒绝（见 [`failure_outcome`]）由缓存层
/// 决定是否回退旧值。
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
    let request_url = url.to_owned();
    swr_cache::read_or_fetch(url, ttl_millis, swr_cache::now_millis, move |etag| async move {
        match conditional_get(&request_url, etag.as_deref()).await {
            Err(error) => FetchOutcome::Transport(error.code().to_owned()),
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
    use super::failure_outcome;
    use crate::api::swr_cache::FetchOutcome;

    #[test]
    fn only_busy_and_gateway_statuses_are_transient_rejections() {
        for status in [429, 502, 503, 504, 520, 521, 522, 523, 524] {
            assert!(
                matches!(failure_outcome::<(), _>(status, "busy"), FetchOutcome::Busy("busy")),
                "{status}"
            );
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
        }
    }
}
