//! Public 拍卖、lot、竞价日志和价格历史查询。

use std::future::Future;

use auction_web_shared::timer::TimeoutFuture;
use gloo_net::http::Response;
use url::form_urlencoded;

use super::swr_cache::{self, CacheRead, FetchOutcome};
use super::{
    AuctionListPage, BidLogPage, CatalogLevelFilter, CurrentAuction, LotEnvelope, PriceHistoryPage,
    PriceHistoryQuery, TransportError, Worlds, decoder, send_get, send_get_conditional,
};

const PUBLIC_QUERY_RETRY_DELAY_MILLIS: u32 = 1_000;

/// 价格历史读取失败。服务端只以 503 表示读端口不可用，不区分状态码：页面统一提示并可重试，
/// 也不把服务端 message、响应正文或底层网络错误带进组件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PriceHistoryFetchError;

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

/// 非 2xx 响应的缓存分类：429（读闸繁忙或来源超额）、503（读端口暂不可用），以及源站不可达时
/// 网关返回的 502/504 与 CDN 的 520–524，都是暂时性拒绝，与断网同样处理：缓存层有可回退的
/// 旧快照时继续展示。其余状态（含无 ETag 却收到的 304、500 与业务拒绝）失败关闭。
fn failure_outcome<T, E>(status: u16, error: E) -> FetchOutcome<T, E> {
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
async fn cached_public_get<T, D, Fut>(url: &str, ttl_millis: u64, decode: D) -> Result<T, String>
where
    T: Clone + 'static,
    D: FnOnce(Response) -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    cached_public_read(url, ttl_millis, decode).await.map(|read| read.value)
}

/// 与 [`cached_public_get`] 相同，但保留“暂时性失败后回退旧快照”的标记，供需要稍后再读的
/// 目录读取使用。
async fn cached_public_read<T, D, Fut>(
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
    let url = current_auction_url(&world_id);
    cached_public_get(&url, swr_cache::LIVE_TTL_MILLIS, move |response| async move {
        Ok(decoder::decode_current_auction(response, &world_id).await?.auction)
    })
    .await
}

/// 当前场次的唯一 URL 构造点：始终用 `include=proxy_default_mode` 显式请求冻结策略的代理默认
/// 模式，解码器据此要求该键。服务端只对显式请求追加该键，旧页面的缺省请求仍得到原字段集合。
fn current_auction_url(world_id: &str) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("world", world_id).append_pair("include", "proxy_default_mode");
    format!("/api/v1/auctions/current?{}", query.finish())
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
    // 未结算的场次（含仍可出价的 live/final）是所在世界唯一的活动场次，从当前场次接口取得
    // 完整上下文，其中的冻结代理默认模式决定代理新建表单的初始选项；已结算场次不再竞价，
    // 只从往期目录拼装，没有该值。
    if status != "settled" {
        return fetch_matching_current_auction(&envelope.auction_id, &status).await;
    }
    let summary =
        fetch_auction_summary(&envelope.auction_id, status.clone(), Some(settled_first_page))
            .await?;
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
        proxy_default_mode: None,
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
            Ok(Some(auction)) if is_target_current(&auction, auction_id, expected_status) => {
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

/// 当前场次条目是否就是按 ID 打开的目标场次。lot 外壳与当前场次各有缓存 TTL，live→final
/// 切换前后两者可能短暂各持一边；同属可出价阶段时以当前场次条目为准，其余状态必须一致。
fn is_target_current(auction: &CurrentAuction, auction_id: &str, expected_status: &str) -> bool {
    auction.id == auction_id
        && (auction.status == expected_status
            || matches!(
                (auction.status.as_str(), expected_status),
                ("live" | "final", "live" | "final")
            ))
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
    if let Ok(CacheRead { value, stale_fallback: false }) = read().await {
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

    use super::super::swr_cache::{CacheRead, FetchOutcome};
    use super::{
        current_auction_url, current_lots_url, failure_outcome, is_target_current,
        retry_once_unless_authoritative,
    };
    use crate::api::{CatalogLevelFilter, CurrentAuction};

    const AUCTION_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

    type CatalogRead = Result<CacheRead<&'static str>, &'static str>;

    fn authoritative(value: &'static str) -> CatalogRead {
        Ok(CacheRead { value, stale_fallback: false })
    }

    fn stale(value: &'static str) -> CatalogRead {
        Ok(CacheRead { value, stale_fallback: true })
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
    fn current_auction_url_always_requests_the_proxy_default_mode() {
        // 看板、外壳、标记页与按场次查找共用这一构造点；解码器要求的键必须由请求显式选择，
        // 不带参数的缺省响应没有该键。
        assert_eq!(
            current_auction_url("main"),
            "/api/v1/auctions/current?world=main&include=proxy_default_mode"
        );
        assert_eq!(
            current_auction_url("isekai"),
            "/api/v1/auctions/current?world=isekai&include=proxy_default_mode"
        );
    }

    fn current_with(status: &str) -> CurrentAuction {
        CurrentAuction {
            id: AUCTION_ID.to_owned(),
            world: "main".to_owned(),
            number: 12,
            status: status.to_owned(),
            opens_at: "2026-11-01T00:00:00Z".to_owned(),
            scheduled_ends_at: Some("2026-11-08T00:00:00Z".to_owned()),
            presentation_mode: "live".to_owned(),
            bidding_open: true,
            preview_revision: 1,
            proxy_default_mode: Some("fast".to_owned()),
        }
    }

    /// 按场次 ID 打开活动场次时，只容忍 lot 外壳与当前场次缓存之间的 live↔final 切换差；其他
    /// 状态差与不同场次都不算命中。
    #[test]
    fn target_current_tolerates_only_the_live_final_switch() {
        for (current, expected) in
            [("live", "live"), ("final", "final"), ("live", "final"), ("final", "live")]
        {
            assert!(
                is_target_current(&current_with(current), AUCTION_ID, expected),
                "{current} vs {expected}"
            );
        }
        for (current, expected) in [("preview", "live"), ("live", "preview"), ("final", "settling")]
        {
            assert!(
                !is_target_current(&current_with(current), AUCTION_ID, expected),
                "{current} vs {expected}"
            );
        }
        assert!(is_target_current(&current_with("preview"), AUCTION_ID, "preview"));
        let other = "0190f2b4-1c2d-4e5f-8a6b-000000000000";
        assert!(!is_target_current(&current_with("live"), other, "live"));
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
