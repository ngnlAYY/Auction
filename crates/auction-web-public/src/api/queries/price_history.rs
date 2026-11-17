//! 价格历史读取；失败统一为不带服务端细节、可重试的错误。

use url::form_urlencoded;

use super::super::swr_cache::{self, FetchOutcome};
use super::super::{PriceHistoryPage, PriceHistoryQuery, decoder};
use super::cached_get::{ConditionalResponse, conditional_get, failure_outcome};

/// 价格历史读取失败。服务端只以 503 表示读端口不可用，不区分状态码：页面统一提示并可重试，
/// 也不把服务端 message、响应正文或底层网络错误带进组件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PriceHistoryFetchError;

pub(crate) async fn fetch_price_history(
    query: PriceHistoryQuery,
    page_size: i64,
) -> Result<PriceHistoryPage, PriceHistoryFetchError> {
    let mut params = form_urlencoded::Serializer::new(String::new());
    for (key, value) in [
        ("world", query.world.as_deref()),
        ("name", query.name.as_deref()),
        ("from", query.from.as_deref()),
        ("to", query.to.as_deref()),
        ("min_price", query.min_price.as_deref()),
        ("max_price", query.max_price.as_deref()),
        ("buyer", query.buyer.as_deref()),
        ("seller", query.seller.as_deref()),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            params.append_pair(key, value);
        }
    }
    params
        .append_pair("sort", &query.sort)
        .append_pair("page", &query.page.max(1).to_string())
        .append_pair("page_size", &page_size.to_string());
    let url = format!("/api/v1/price-history?{}", params.finish());
    let request_url = url.clone();
    swr_cache::get_or_fetch(
        &url,
        swr_cache::CATALOG_TTL_MILLIS,
        swr_cache::now_millis,
        move |etag| async move {
            match conditional_get(&request_url, etag.as_deref()).await {
                Err(_) => FetchOutcome::Transport(PriceHistoryFetchError),
                Ok(ConditionalResponse::NotModified) => FetchOutcome::NotModified,
                Ok(ConditionalResponse::Failure(response)) => {
                    failure_outcome(response.status(), PriceHistoryFetchError)
                }
                Ok(ConditionalResponse::Success { response, etag }) => {
                    match decoder::decode_price_history_page(response).await {
                        Ok(value) => FetchOutcome::Changed { value, etag },
                        Err(_) => FetchOutcome::Rejected(PriceHistoryFetchError),
                    }
                }
            }
        },
    )
    .await
}
