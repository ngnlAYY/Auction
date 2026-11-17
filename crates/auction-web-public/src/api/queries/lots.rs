//! lot 目录读取（首次未得到权威值时的单次重读、首屏预取与唯一 URL 构造点）与 lot 详情读取。

use std::future::Future;

use auction_web_shared::timer::TimeoutFuture;
use url::form_urlencoded;

use super::super::swr_cache::{self, CacheRead, FetchOutcome};
use super::super::{CatalogLevelFilter, LotEnvelope, decoder};
use super::cached_get::{
    ConditionalResponse, cached_public_read, conditional_get, failure_outcome,
};

const PUBLIC_QUERY_RETRY_DELAY_MILLIS: u32 = 1_000;

/// lot 详情读取失败。详情只在页内抽屉展示：失败统一提示并可重试，不区分状态码，也不把
/// 服务端 message、响应正文或底层网络错误带进组件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LotDetailFetchError;

pub(crate) async fn fetch_current_lots(
    auction_id: String,
    category: Option<String>,
    equipment_group: Option<String>,
    levels: CatalogLevelFilter,
    page: i64,
    page_size: i64,
) -> Result<LotEnvelope, String> {
    retry_once_unless_authoritative(
        || {
            fetch_current_lots_once(
                &auction_id,
                category.as_deref(),
                equipment_group.as_deref(),
                levels,
                page,
                page_size,
            )
        },
        || TimeoutFuture::new(PUBLIC_QUERY_RETRY_DELAY_MILLIS),
    )
    .await
}

/// 旧 Public QueryClient 对目录读取固定再尝试一次，错误壳只在第二次失败后出现。
/// 首次读到权威值（新鲜命中、304 确认或新响应）直接返回；首次失败，或只得到传输失败、暂时性拒绝
/// 暂时性拒绝后回退的旧快照时，等待后重读一次：等待期间 Resource 保留已展示的目录，拥塞通常
/// 已过，能及时追上价格与领先者。第二次结果原样返回：再次回退时接受旧快照，失败关闭的拒绝照常
/// 报错。
/// `wait` 在首次读取结束后才构造，浏览器定时器从那一刻起计时。
async fn retry_once_unless_authoritative<T, E, R, RF, W, WF>(read: R, wait: W) -> Result<T, E>
where
    R: Fn() -> RF,
    RF: Future<Output = Result<CacheRead<T>, E>>,
    W: FnOnce() -> WF,
    WF: Future<Output = ()>,
{
    if let Ok(CacheRead { value, stale_fallback: None }) = read().await {
        return Ok(value);
    }
    wait().await;
    read().await.map(|cached| cached.value)
}

/// 首屏预取：与 `/auctions/current` 并行读取上次记住的场次 lots，只单次尝试、丢弃结果。
/// 目录随后以相同参数读取时由缓存层复用这次在途或已写入的结果；场次已变化时只多一次请求。
pub(crate) async fn prefetch_current_lots(
    auction_id: String,
    category: Option<String>,
    equipment_group: Option<String>,
    levels: CatalogLevelFilter,
    page: i64,
    page_size: i64,
) {
    let _ = fetch_current_lots_once(
        &auction_id,
        category.as_deref(),
        equipment_group.as_deref(),
        levels,
        page,
        page_size,
    )
    .await;
}

async fn fetch_current_lots_once(
    auction_id: &str,
    category: Option<&str>,
    equipment_group: Option<&str>,
    levels: CatalogLevelFilter,
    page: i64,
    page_size: i64,
) -> Result<CacheRead<LotEnvelope>, String> {
    let url = current_lots_url(auction_id, category, equipment_group, levels, page, page_size);
    let expected_auction = auction_id.to_owned();
    let expected_category = category.map(str::to_owned);
    let expected_equipment_group = equipment_group.map(str::to_owned);
    cached_public_read(&url, swr_cache::LIVE_TTL_MILLIS, move |response| async move {
        decoder::decode_lot_list(
            response,
            &expected_auction,
            expected_category.as_deref(),
            expected_equipment_group.as_deref(),
            levels,
        )
        .await
    })
    .await
}

/// lot 目录 URL 的唯一构造点：读取、首屏预取、标记页与历史场次都经这里，同一组参数得到逐字
/// 相同的 URL，缓存层才能按 URL 合并在途请求。目录行不展示装备属性，始终请求精简投影
/// （`fields=summary`）；属性明细只在打开详情抽屉时经 lot 详情读取。
fn current_lots_url(
    auction_id: &str,
    category: Option<&str>,
    equipment_group: Option<&str>,
    levels: CatalogLevelFilter,
    page: i64,
    page_size: i64,
) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.to_string()).append_pair("page_size", &page_size.to_string());
    if let Some(category) = category {
        query.append_pair("category", category);
    }
    if let Some(equipment_group) = equipment_group {
        query.append_pair("equipment_group", equipment_group);
    }
    if let Some(maximum) = levels.max_level {
        query.append_pair("max_level", &maximum.to_string());
    }
    if !levels.include_unleveled {
        query.append_pair("include_unleveled", "false");
    }
    query.append_pair("fields", "summary");
    format!("/api/v1/auctions/{auction_id}/lots?{}", query.finish())
}

pub(crate) async fn fetch_lot_detail(lot_id: String) -> Result<LotEnvelope, LotDetailFetchError> {
    let url = format!("/api/v1/lots/{lot_id}");
    let request_url = url.clone();
    swr_cache::get_or_fetch(
        &url,
        swr_cache::LIVE_TTL_MILLIS,
        swr_cache::now_millis,
        move |etag| async move {
            match conditional_get(&request_url, etag.as_deref()).await {
                Err(_) => FetchOutcome::Transport(LotDetailFetchError),
                Ok(ConditionalResponse::NotModified) => FetchOutcome::NotModified,
                Ok(ConditionalResponse::Failure(response)) => {
                    failure_outcome(response.status(), LotDetailFetchError)
                }
                Ok(ConditionalResponse::Success { response, etag }) => {
                    match decoder::decode_lot_detail(response, None, &lot_id).await {
                        Ok(value) => FetchOutcome::Changed { value, etag },
                        Err(_) => FetchOutcome::Rejected(LotDetailFetchError),
                    }
                }
            }
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use futures_util::FutureExt;

    use super::{current_lots_url, retry_once_unless_authoritative};
    use crate::api::CatalogLevelFilter;
    use crate::api::swr_cache::CacheRead;

    const AUCTION_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

    type CatalogRead = Result<CacheRead<&'static str>, &'static str>;

    fn authoritative(value: &'static str) -> CatalogRead {
        Ok(CacheRead { value, stale_fallback: None })
    }

    fn stale(value: &'static str) -> CatalogRead {
        Ok(CacheRead { value, stale_fallback: Some(1_000) })
    }

    /// 依次给出每次读取的结果，返回最终结果、读取次数与等待次数；读取与等待都立即就绪。
    fn run_catalog_retry(
        reads: impl IntoIterator<Item = CatalogRead>,
    ) -> (Result<&'static str, &'static str>, u32, u32) {
        let reads = RefCell::new(reads.into_iter());
        let read_count = Cell::new(0);
        let wait_count = Cell::new(0);
        let result = retry_once_unless_authoritative(
            || {
                read_count.set(read_count.get() + 1);
                std::future::ready(reads.borrow_mut().next().unwrap_or(Err("unexpected_read")))
            },
            || {
                wait_count.set(wait_count.get() + 1);
                std::future::ready(())
            },
        )
        .now_or_never()
        .unwrap_or(Err("pending"));
        (result, read_count.get(), wait_count.get())
    }

    #[test]
    fn catalog_read_retries_once_after_a_stale_fallback() {
        // 首次读到权威值：直接返回，不等待、不重读。
        assert_eq!(run_catalog_retry([authoritative("v2")]), (Ok("v2"), 1, 0));
        // 首次只得到暂时性失败回退的旧快照：等待后重读，追上权威值。
        assert_eq!(run_catalog_retry([stale("v1"), authoritative("v2")]), (Ok("v2"), 2, 1));
        // 第二次仍是回退：接受旧快照，不再重试。
        assert_eq!(run_catalog_retry([stale("v1"), stale("v1")]), (Ok("v1"), 2, 1));
        // 第二次失败关闭：返回错误，不拿首次的旧快照兜底。
        assert_eq!(run_catalog_retry([stale("v1"), Err("not_found")]), (Err("not_found"), 2, 1));
        // 首次失败（没有可回退的快照）：照旧再读一次。
        assert_eq!(run_catalog_retry([Err("read_busy"), authoritative("v1")]), (Ok("v1"), 2, 1));
    }

    #[test]
    fn lot_list_url_always_requests_the_summary_projection() {
        // 读取、预取、标记页与历史场次共用这一构造点：同一组参数得到逐字相同的 URL。
        assert_eq!(
            current_lots_url(AUCTION_ID, None, None, CatalogLevelFilter::default(), 1, 20),
            format!("/api/v1/auctions/{AUCTION_ID}/lots?page=1&page_size=20&fields=summary")
        );
        let levels = CatalogLevelFilter { max_level: Some(450), include_unleveled: false };
        assert_eq!(
            current_lots_url(AUCTION_ID, Some("clo"), Some("rare"), levels, 3, 50),
            format!(
                "/api/v1/auctions/{AUCTION_ID}/lots?page=3&page_size=50&category=clo\
                 &equipment_group=rare&max_level=450&include_unleveled=false&fields=summary"
            )
        );
    }
}
