//! Public 拍卖、lot、竞价日志和价格历史查询。

use super::swr_cache::{self, FetchOutcome};
use super::{
    AuctionListPage, BidLogPage, CatalogLevelFilter, CurrentAuction, LotEnvelope, PriceHistoryPage,
    PriceHistoryQuery, TransportError, Worlds, decoder, send_get, send_get_conditional,
};
use auction_web_shared::timer::TimeoutFuture;
use gloo_net::http::Response;
use std::future::Future;
use url::form_urlencoded;

const PUBLIC_QUERY_RETRY_DELAY_MILLIS: u32 = 1_000;

/// 价格历史读取失败的浏览器安全分类；页面只区分永久能力缺失和可重试故障。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PriceHistoryFetchError {
    Unsupported,
    Unavailable,
}

/// lot 详情读取失败。详情只在页内抽屉展示：失败统一提示并可重试，不区分状态码，也不把
/// 服务端 message、响应正文或底层网络错误带进组件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LotDetailFetchError;

/// 条件 GET 的响应分类；304 只在确实发送了 `If-None-Match` 时成立。
enum ConditionalResponse {
    NotModified,
    Success { response: Response, etag: Option<String> },
    Failure(Response),
}

async fn conditional_get(
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

/// 对公开只读 GET 执行带缓存的严格解码。
/// `decode` 必须完成全部跨字段校验（含响应与请求身份的绑定）；只有它返回 Ok 的正文才写入
/// 缓存，4xx/5xx 或校验失败不会留下条目。传输错误由缓存层决定是否回退旧值。
async fn cached_public_get<T, D, Fut>(url: &str, ttl_millis: u64, decode: D) -> Result<T, String>
where
    T: Clone + 'static,
    D: FnOnce(Response) -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let request_url = url.to_owned();
    swr_cache::get_or_fetch(url, ttl_millis, swr_cache::now_millis, move |etag| async move {
        match conditional_get(&request_url, etag.as_deref()).await {
            Err(error) => FetchOutcome::Transport(error.code().to_owned()),
            Ok(ConditionalResponse::NotModified) => FetchOutcome::NotModified,
            Ok(ConditionalResponse::Failure(response)) => {
                FetchOutcome::Rejected(decoder::decode_error_code(response).await)
            }
            Ok(ConditionalResponse::Success { response, etag }) => match decode(response).await {
                Ok(value) => FetchOutcome::Changed { value, etag },
                Err(error) => FetchOutcome::Rejected(error),
            },
        }
    })
    .await
}

pub(crate) async fn fetch_worlds() -> Result<Worlds, String> {
    let response = send_get("/api/v1/worlds").await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_worlds(response).await
}

pub(crate) async fn fetch_current_auction(
    world_id: String,
) -> Result<Option<CurrentAuction>, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("world", &world_id);
    let encoded_world = query.finish();
    let url = format!("/api/v1/auctions/current?{encoded_world}");
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, move |response| async move {
        Ok(decoder::decode_current_auction(response, &world_id).await?.auction)
    })
    .await
}

/// 读取指定历史拍卖的公开投影。
/// 旧 v1 lot wire 不包含世界、场次号和计划结束时间；这些展示上下文必须从拍卖目录
/// 读取，不能通过扩张 lot 响应破坏旧客户端的严格字段闭集。
pub(crate) async fn fetch_historical_auction(auction_id: String) -> Result<CurrentAuction, String> {
    // 目录与看板默认目录共用同一 URL（page_size=20），命中 SWR 缓存，避免点击详情时
    // 额外触发一次 10 条目录请求并在返回时闪现加载态。按场次 ID 打开的页面绝大多数是已结算
    // 场次：已结算目录第一页与 lot 目录同时发出，省去“等状态再查摘要”的一轮串行往返；
    // 该 URL 是公开低频缓存档，状态不是 settled 时丢弃预取结果、按实际状态查找。
    let (envelope, settled_first_page) = futures_util::join!(
        fetch_current_lots(auction_id, None, None, CatalogLevelFilter::default(), 1, 20),
        fetch_auction_page(None, "settled".to_owned(), 1, SUMMARY_PAGE_SIZE),
    );
    let envelope = envelope?;
    let status = envelope.auction_status.clone();
    if !matches!(status.as_str(), "live" | "final" | "settled") {
        return fetch_matching_current_auction(&envelope.auction_id, &status).await;
    }
    let first_page = (status == "settled").then_some(settled_first_page);
    let summary = fetch_auction_summary(&envelope.auction_id, status.clone(), first_page).await?;
    Ok(CurrentAuction {
        id: envelope.auction_id,
        world: summary.world_id,
        number: summary.sequence,
        status,
        opens_at: envelope.opens_at,
        scheduled_ends_at: summary.effective_end_at,
        presentation_mode: envelope.presentation_mode,
        bidding_open: envelope.bidding_open,
        preview_revision: envelope.preview_revision,
    })
}

/// 历史场次摘要的目录页大小；与并行预取的第一页使用同一 URL，命中同一缓存条目。
const SUMMARY_PAGE_SIZE: i64 = 200;

/// 在指定状态的场次目录中查找摘要；`first_page` 是调用方已并行取得的第一页。
async fn fetch_auction_summary(
    auction_id: &str,
    status: String,
    mut first_page: Option<Result<AuctionListPage, String>>,
) -> Result<super::HistoricalAuction, String> {
    const PAGE_SIZE: i64 = SUMMARY_PAGE_SIZE;
    let mut page_number = 1_i64;
    loop {
        let page = match first_page.take() {
            Some(page) => page?,
            None => fetch_auction_page(None, status.clone(), page_number, PAGE_SIZE).await?,
        };
        let row_count =
            i64::try_from(page.auctions.len()).map_err(|_| "拍卖目录页过大".to_owned())?;
        let total = page.total;
        if let Some(auction) = page.auctions.into_iter().find(|auction| auction.id == auction_id) {
            return Ok(auction);
        }
        if row_count < PAGE_SIZE
            || total.is_some_and(|total| page_number.saturating_mul(PAGE_SIZE) >= total)
        {
            return Err("拍卖目录中不存在目标拍卖".to_owned());
        }
        page_number = page_number.checked_add(1).ok_or_else(|| "拍卖目录页码溢出".to_owned())?;
    }
}

async fn fetch_matching_current_auction(
    auction_id: &str,
    expected_status: &str,
) -> Result<CurrentAuction, String> {
    let worlds = fetch_worlds().await?;
    // 各世界的当前场次互不依赖，并行读取：任一世界命中即返回；都未命中时先报读取失败，
    // 不把读不到的世界当作“不存在目标拍卖”。
    let currents =
        futures_util::future::join_all(worlds.worlds.into_keys().map(fetch_current_auction)).await;
    let mut first_error = None;
    for current in currents {
        match current {
            Ok(Some(auction)) if auction.id == auction_id && auction.status == expected_status => {
                return Ok(auction);
            }
            Ok(_) => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "当前拍卖目录中不存在目标拍卖".to_owned()))
}

pub(crate) async fn fetch_bid_log_page(
    auction_id: String,
    page: i64,
    world_id: String,
    lot_id: Option<String>,
    page_size: i64,
) -> Result<BidLogPage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", &page_size.to_string());
    if !world_id.is_empty() {
        query.append_pair("world", &world_id);
    }
    if let Some(lot_id) = lot_id {
        query.append_pair("lot", &lot_id);
    }
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{}", query.finish());
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, |response| async move {
        decoder::decode_bid_log_page(response).await
    })
    .await
}

/// 读取可供公开竞价日志选择的 live、final 和 settled 拍卖，保留已结算场次的历史深链。
pub(crate) async fn fetch_bid_log_auctions(
    world_id: String,
) -> Result<Vec<super::HistoricalAuction>, String> {
    // 状态目录相互独立，最多并发三个读取；各状态内部仍按编号顺序逐页读取。
    let (mut auctions, final_auctions, settled_auctions) = futures_util::try_join!(
        fetch_bid_log_auction_status(&world_id, "live"),
        fetch_bid_log_auction_status(&world_id, "final"),
        fetch_bid_log_auction_status(&world_id, "settled"),
    )?;
    auctions.extend(final_auctions);
    auctions.extend(settled_auctions);
    auctions.sort_by(|left, right| {
        right.sequence.cmp(&left.sequence).then_with(|| right.id.cmp(&left.id))
    });
    auctions.dedup_by(|left, right| left.id == right.id);
    Ok(auctions)
}

async fn fetch_bid_log_auction_status(
    world_id: &str,
    status: &str,
) -> Result<Vec<super::HistoricalAuction>, String> {
    let mut auctions = Vec::new();
    let mut page_number = 1_i64;
    loop {
        let page =
            fetch_auction_page(Some(world_id.to_owned()), status.to_owned(), page_number, 200)
                .await?;
        let row_count =
            i64::try_from(page.auctions.len()).map_err(|_| "拍卖目录页过大".to_owned())?;
        let page_size = page.page_size;
        if page_size <= 0 {
            return Err("拍卖目录分页大小无效".to_owned());
        }
        let total = page.total;
        auctions.extend(page.auctions);
        // 满页且缺 total 时继续读取，避免选择器静默截断到前 200 条。
        if row_count < page_size
            || total.is_some_and(|total| page_number.saturating_mul(page_size) >= total)
        {
            return Ok(auctions);
        }
        page_number = page_number.checked_add(1).ok_or_else(|| "拍卖目录页码溢出".to_owned())?;
    }
}

/// 读取单个 lot 的编号竞价历史；分页参数沿用公开竞价日志契约。
pub(crate) async fn fetch_lot_bid_log(
    auction_id: String,
    lot_id: String,
    page: i64,
    page_size: i64,
) -> Result<BidLogPage, String> {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("lot", &lot_id)
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", &page_size.to_string())
        .finish();
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{query}");
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, |response| async move {
        decoder::decode_bid_log_page(response).await
    })
    .await
}

async fn fetch_auction_page(
    world_id: Option<String>,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AuctionListPage, String> {
    let world =
        world_id.as_deref().filter(|world| !world.is_empty() && *world != "all").map(str::to_owned);
    let url = auction_page_url("/api/v1/auctions", world.as_deref(), &status, page, page_size);
    cached_public_get(&url, swr_cache::CATALOG_TTL_MILLIS, move |response| async move {
        let decoded = decoder::decode_auction_list(response, world.as_deref(), &status).await?;
        bind_auction_page(decoded, page, page_size)
    })
    .await
}

/// 历史页用 v2 一次取得本页摘要与论坛地址，不按行加载完整论坛镜像。
pub(crate) async fn fetch_auction_history_page(
    world_id: Option<String>,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AuctionListPage<super::HistoricalAuctionWithForum>, String> {
    let world =
        world_id.as_deref().filter(|world| !world.is_empty() && *world != "all").map(str::to_owned);
    let url = auction_page_url("/api/v2/auctions", world.as_deref(), &status, page, page_size);
    cached_public_get(&url, swr_cache::CATALOG_TTL_MILLIS, move |response| async move {
        let decoded = decoder::decode_auction_list_v2(response, world.as_deref(), &status).await?;
        bind_auction_page(decoded, page, page_size)
    })
    .await
}

/// 页面和跨页查找共用请求身份校验；先完成严格解码，不能把错页响应拼进目录。
fn bind_auction_page<T>(
    response: AuctionListPage<T>,
    page: i64,
    page_size: i64,
) -> Result<AuctionListPage<T>, String> {
    if response.page != Some(page) || response.page_size != page_size {
        return Err("invalid_response".to_owned());
    }
    Ok(response)
}

fn auction_page_url(
    endpoint: &str,
    world: Option<&str>,
    status: &str,
    page: i64,
    page_size: i64,
) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.to_string())
        .append_pair("page_size", &page_size.to_string())
        .append_pair("status", status);
    if let Some(world) = world {
        query.append_pair("world", world);
    }
    format!("{endpoint}?{}", query.finish())
}

/// 预览切换只消费同一世界的近期结算场次，避免被其他世界的前 200 条挤出目录。
pub(crate) async fn fetch_auction_list(
    world_id: String,
) -> Result<Vec<super::HistoricalAuction>, String> {
    Ok(fetch_auction_page(Some(world_id), "settled".to_owned(), 1, 200).await?.auctions)
}

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
                Err(_) => FetchOutcome::Transport(PriceHistoryFetchError::Unavailable),
                Ok(ConditionalResponse::NotModified) => FetchOutcome::NotModified,
                Ok(ConditionalResponse::Failure(response)) => {
                    let status = response.status();
                    // 与命令路径一致走共享错误 envelope 解码；无效 envelope 归一为 invalid_response。
                    let code = decoder::decode_error_code(response).await;
                    FetchOutcome::Rejected(if status == 501 && code == "feature_not_supported" {
                        PriceHistoryFetchError::Unsupported
                    } else {
                        PriceHistoryFetchError::Unavailable
                    })
                }
                Ok(ConditionalResponse::Success { response, etag }) => {
                    match decoder::decode_price_history_page(response).await {
                        Ok(value) => FetchOutcome::Changed { value, etag },
                        Err(_) => FetchOutcome::Rejected(PriceHistoryFetchError::Unavailable),
                    }
                }
            }
        },
    )
    .await
}

pub(crate) async fn fetch_current_lots(
    auction_id: String,
    category: Option<String>,
    equipment_group: Option<String>,
    levels: CatalogLevelFilter,
    page: i64,
    page_size: i64,
) -> Result<LotEnvelope, String> {
    if let Ok(lots) = fetch_current_lots_once(
        &auction_id,
        category.as_deref(),
        equipment_group.as_deref(),
        levels,
        page,
        page_size,
    )
    .await
    {
        return Ok(lots);
    }
    // 旧 Public QueryClient 对目录读取固定再尝试一次；错误壳只在第二次失败后出现。
    TimeoutFuture::new(PUBLIC_QUERY_RETRY_DELAY_MILLIS).await;
    fetch_current_lots_once(
        &auction_id,
        category.as_deref(),
        equipment_group.as_deref(),
        levels,
        page,
        page_size,
    )
    .await
}

async fn fetch_current_lots_once(
    auction_id: &str,
    category: Option<&str>,
    equipment_group: Option<&str>,
    levels: CatalogLevelFilter,
    page: i64,
    page_size: i64,
) -> Result<LotEnvelope, String> {
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
    let url = format!("/api/v1/auctions/{auction_id}/lots?{}", query.finish());
    let expected_auction = auction_id.to_owned();
    let expected_category = category.map(str::to_owned);
    let expected_equipment_group = equipment_group.map(str::to_owned);
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, move |response| async move {
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
                Ok(ConditionalResponse::Failure(_)) => FetchOutcome::Rejected(LotDetailFetchError),
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
