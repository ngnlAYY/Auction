//! Public 拍卖、lot、竞价日志和价格历史查询。

use super::{
    AuctionListPage, BidLogPage, CurrentAuction, LotEnvelope, PriceHistoryPage, PriceHistoryQuery,
    TransportError, Worlds, decoder, send_get,
};
use gloo_timers::future::TimeoutFuture;
use url::form_urlencoded;

const PUBLIC_QUERY_RETRY_DELAY_MILLIS: u32 = 1_000;

/// 价格历史读取失败的浏览器安全分类；页面只区分永久能力缺失和可重试故障。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PriceHistoryFetchError {
    Unsupported,
    Unavailable,
}

/// lot 详情读取失败的浏览器安全投影。
///
/// 旧页面按 HTTP status 与稳定 code 选择本地化文案；这里不保存服务端 message、响应正文或
/// 底层网络错误，避免把不受信任内容直接带进组件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LotDetailFetchError {
    status: u16,
    code: String,
}

impl LotDetailFetchError {
    pub(crate) const fn status(&self) -> u16 {
        self.status
    }

    pub(crate) fn code(&self) -> &str {
        &self.code
    }

    fn transport(error: TransportError) -> Self {
        Self { status: 0, code: error.code().to_owned() }
    }

    fn invalid_response(status: u16) -> Self {
        Self { status, code: "invalid_response".to_owned() }
    }
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
    let response = send_get(&url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    Ok(decoder::decode_current_auction(response, &world_id).await?.auction)
}

/// 读取指定历史拍卖的公开投影。
///
/// 旧 v1 lot wire 不包含世界、场次号和计划结束时间；这些展示上下文必须从拍卖目录
/// 读取，不能通过扩张 lot 响应破坏旧客户端的严格字段闭集。
pub(crate) async fn fetch_historical_auction(auction_id: String) -> Result<CurrentAuction, String> {
    let envelope = fetch_current_lots(auction_id, None, 1).await?;
    let status = envelope.auction_status.clone();
    if !matches!(status.as_str(), "live" | "final" | "settled") {
        return fetch_matching_current_auction(&envelope.auction_id, &status).await;
    }
    let summary = fetch_auction_summary(&envelope.auction_id, status.clone()).await?;
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

async fn fetch_auction_summary(
    auction_id: &str,
    status: String,
) -> Result<super::HistoricalAuction, String> {
    const PAGE_SIZE: i64 = 200;
    let mut page_number = 1_i64;
    loop {
        let page = fetch_auction_page(None, status.clone(), page_number, PAGE_SIZE).await?;
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
    for world_id in worlds.worlds.into_keys() {
        if let Some(auction) = fetch_current_auction(world_id).await?
            && auction.id == auction_id
            && auction.status == expected_status
        {
            return Ok(auction);
        }
    }
    Err("当前拍卖目录中不存在目标拍卖".to_owned())
}

pub(crate) async fn fetch_bid_log_page(
    auction_id: String,
    page: i64,
    world_id: String,
    lot_id: Option<String>,
) -> Result<BidLogPage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string()).append_pair("page_size", "10");
    if !world_id.is_empty() {
        query.append_pair("world", &world_id);
    }
    if let Some(lot_id) = lot_id {
        query.append_pair("lot", &lot_id);
    }
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{}", query.finish());
    let response = send_get(&url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_bid_log_page(response).await
}

/// 读取可供公开竞价日志选择的拍卖；旧版只展示 live、final 和 settled 拍卖。
pub(crate) async fn fetch_bid_log_auctions(
    world_id: String,
) -> Result<Vec<super::HistoricalAuction>, String> {
    let mut auctions = Vec::new();
    for status in ["live", "final", "settled"] {
        let page = fetch_auction_page(Some(world_id.clone()), status.to_owned(), 1, 200).await?;
        auctions.extend(page.auctions);
    }
    auctions.sort_by(|left, right| {
        right.sequence.cmp(&left.sequence).then_with(|| right.id.cmp(&left.id))
    });
    auctions.dedup_by(|left, right| left.id == right.id);
    Ok(auctions)
}

/// 读取单个 lot 的编号竞价历史；分页参数沿用公开竞价日志契约。
pub(crate) async fn fetch_lot_bid_log(
    auction_id: String,
    lot_id: String,
    page: i64,
) -> Result<BidLogPage, String> {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("lot", &lot_id)
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", "10")
        .finish();
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{query}");
    let response = send_get(&url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_bid_log_page(response).await
}

pub(crate) async fn fetch_auction_page(
    world_id: Option<String>,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AuctionListPage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.to_string())
        .append_pair("page_size", &page_size.to_string())
        .append_pair("status", &status);
    let world_id = world_id.filter(|world| !world.is_empty() && world != "all");
    if let Some(world_id) = world_id.as_deref() {
        query.append_pair("world", world_id);
    }
    let url = format!("/api/v1/auctions?{}", query.finish());
    let response = send_get(&url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_auction_list(response, world_id.as_deref(), &status).await
}

pub(crate) async fn fetch_auction_list() -> Result<Vec<super::HistoricalAuction>, String> {
    Ok(fetch_auction_page(None, "settled".to_owned(), 1, 200).await?.auctions)
}

pub(crate) async fn fetch_price_history(
    query: PriceHistoryQuery,
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
    params.append_pair("page", &query.page.max(1).to_string()).append_pair("page_size", "10");
    let url = format!("/api/v1/price-history?{}", params.finish());
    let response = send_get(&url).await.map_err(|_| PriceHistoryFetchError::Unavailable)?;
    if !response.ok() {
        let status = response.status();
        // 与命令路径一致走共享错误 envelope 解码；无效 envelope 归一为 invalid_response。
        let code = decoder::decode_error_code(response).await;
        return Err(if status == 501 && code == "feature_not_supported" {
            PriceHistoryFetchError::Unsupported
        } else {
            PriceHistoryFetchError::Unavailable
        });
    }
    decoder::decode_price_history_page(response)
        .await
        .map_err(|_| PriceHistoryFetchError::Unavailable)
}

pub(crate) async fn fetch_current_lots(
    auction_id: String,
    category: Option<String>,
    page: i64,
) -> Result<LotEnvelope, String> {
    if let Ok(lots) = fetch_current_lots_once(&auction_id, category.as_deref(), page).await {
        return Ok(lots);
    }
    // 旧 Public QueryClient 对目录读取固定再尝试一次；错误壳只在第二次失败后出现。
    TimeoutFuture::new(PUBLIC_QUERY_RETRY_DELAY_MILLIS).await;
    fetch_current_lots_once(&auction_id, category.as_deref(), page).await
}

async fn fetch_current_lots_once(
    auction_id: &str,
    category: Option<&str>,
    page: i64,
) -> Result<LotEnvelope, String> {
    let category_query = category
        .map(|value| {
            let encoded = form_urlencoded::Serializer::new(String::new())
                .append_pair("category", value)
                .finish();
            format!("&{encoded}")
        })
        .unwrap_or_default();
    let url =
        format!("/api/v1/auctions/{auction_id}/lots?page={page}&page_size=10{category_query}");
    let response = send_get(&url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_lot_list(response, auction_id, category).await
}

pub(crate) async fn fetch_lot_detail(lot_id: String) -> Result<LotEnvelope, LotDetailFetchError> {
    let url = format!("/api/v1/lots/{lot_id}");
    let response = send_get(&url).await.map_err(LotDetailFetchError::transport)?;
    let status = response.status();
    if !response.ok() {
        let code = decoder::decode_error_code(response).await;
        return Err(LotDetailFetchError { status, code });
    }
    decoder::decode_lot_detail(response, None, &lot_id)
        .await
        .map_err(|_| LotDetailFetchError::invalid_response(status))
}
