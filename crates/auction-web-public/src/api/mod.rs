//! Public 浏览器端的同源 HTTP 数据访问。
//!
//! 这里仅解码稳定的 HTTP DTO，不引入数据库模型或应用层端口。请求失败会作为页面状态返回，
//! 组件可以显示可本地化的加载/错误文案，而不是继续渲染迁移阶段的示例数据。

mod account;
mod blacklist;
mod commands;
mod compensation;
mod forum;
mod realtime;
mod rules;
mod session;

pub(crate) use account::{MyBidsPage, OwnedBid, OwnedProxy, fetch_my_bids, fetch_my_proxy};
pub(crate) use blacklist::{BlacklistEntry, BlacklistPage, fetch_blacklist};
pub(crate) use commands::{
    CommandError, ProxyCommandAuthorization, create_proxy_authorization, lower_proxy_authorization,
    place_bid,
};
pub(crate) use compensation::{
    CompensationMovement, CompensationMovementPage, CompensationPoolPage, CompensationRound,
    fetch_compensation_movements, fetch_compensation_pool,
};
pub(crate) use forum::{ForumThread, fetch_forum_thread};
pub(crate) use realtime::{
    HealthStatus, RealtimeEvent, RealtimePage, fetch_health, fetch_realtime_events,
    subscribe_auction_realtime, subscribe_realtime,
};
pub(crate) use rules::{RulesDocument, fetch_rules};
pub(crate) use session::{
    PublicSession, PublicSessionApiError, fetch_public_session, login_public, logout_public,
};

use chrono::{DateTime, Utc};
use gloo_net::http::{Request, Response};
use serde::Deserialize;
use std::collections::BTreeMap;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use url::form_urlencoded;

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

#[derive(Debug, Clone, Deserialize)]
struct ApiResponse<T> {
    data: T,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct CurrentAuction {
    pub id: String,
    pub world: String,
    pub number: i64,
    pub status: String,
    pub opens_at: String,
    pub scheduled_ends_at: Option<String>,
    pub presentation_mode: String,
    #[serde(default)]
    pub bidding_open: bool,
    #[serde(default)]
    pub preview_revision: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct WorldStatus {
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct Worlds {
    pub worlds: BTreeMap<String, WorldStatus>,
}

#[derive(Debug, Clone, Deserialize)]
struct CurrentAuctionData {
    auction: Option<CurrentAuction>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct HistoricalAuction {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub scheduled_open_at: String,
    pub opened_at: Option<String>,
    pub final_phase_at: Option<String>,
    pub effective_end_at: Option<String>,
    pub settled_at: Option<String>,
    pub lot_count: i64,
    pub sold_count: i64,
    pub unsold_count: i64,
    pub sold_amount: i64,
    pub state_version: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct AuctionListData {
    auctions: Vec<HistoricalAuction>,
    #[serde(default)]
    page: Option<i64>,
    #[serde(default)]
    page_size: i64,
    #[serde(default)]
    total: Option<i64>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    has_more: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AuctionListPage {
    pub auctions: Vec<HistoricalAuction>,
    pub page: Option<i64>,
    pub page_size: i64,
    pub total: Option<i64>,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub has_more: Option<bool>,
}

impl From<AuctionListData> for AuctionListPage {
    fn from(data: AuctionListData) -> Self {
        Self {
            auctions: data.auctions,
            page: data.page,
            page_size: data.page_size,
            total: data.total,
            cursor: data.cursor,
            next_cursor: data.next_cursor,
            has_more: data.has_more,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct LotEnvelope {
    pub auction_id: String,
    pub world: String,
    pub number: i64,
    pub auction_status: String,
    pub projection_kind: String,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub opens_at: String,
    pub scheduled_ends_at: Option<String>,
    pub preview_revision: i64,
    pub lots: Option<LotItems>,
    pub lot: Option<LotDetailItem>,
    pub total: Option<i64>,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub page_size: i64,
    pub category_counts: Option<BTreeMap<String, i64>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum LotItems {
    Preview(Vec<PreviewLot>),
    Live(Vec<LiveLot>),
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct PreviewLot {
    pub id: String,
    pub code: String,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub equipment_name: String,
    #[serde(default)]
    pub equipment_category: String,
    pub equipment_level: Option<i64>,
    pub avg_roll: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct EquipmentStat {
    pub value: String,
    pub base: Option<f64>,
    pub roll: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct EquipmentDetail {
    pub name: String,
    pub category: String,
    pub level: Option<i64>,
    #[serde(default)]
    pub info_summary: String,
    #[serde(default)]
    pub stats: BTreeMap<String, EquipmentStat>,
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct LiveLot {
    pub id: String,
    pub lot_number: i64,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    pub next_bid: i64,
    pub has_leader: bool,
    pub leader_name: Option<String>,
    pub seller_name: String,
    pub effective_end_at: Option<String>,
    pub equipment: EquipmentDetail,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct PreviewLotDetail {
    #[serde(flatten)]
    pub summary: PreviewLot,
    pub equipment: EquipmentDetail,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum LotDetailItem {
    Preview(PreviewLotDetail),
    Live(LiveLot),
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct BidLogEntry {
    pub id: String,
    pub lot_id: String,
    pub lot_number: i64,
    pub amount: i64,
    pub equipment_name: String,
    #[serde(default)]
    pub equipment_category: String,
    pub bidder_name: String,
    pub valid: bool,
    pub mechanism: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct BidLogData {
    bids: Vec<BidLogEntry>,
    page: Option<i64>,
    page_size: i64,
    total: Option<i64>,
    cursor: Option<String>,
    next_cursor: Option<String>,
    has_more: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct BidLogPage {
    pub bids: Vec<BidLogEntry>,
    pub page: Option<i64>,
    pub page_size: i64,
    pub total: Option<i64>,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub has_more: Option<bool>,
}

impl From<BidLogData> for BidLogPage {
    fn from(data: BidLogData) -> Self {
        Self {
            bids: data.bids,
            page: data.page,
            page_size: data.page_size,
            total: data.total,
            cursor: data.cursor,
            next_cursor: data.next_cursor,
            has_more: data.has_more,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct PriceHistoryEntry {
    pub lot_id: String,
    pub auction_id: String,
    pub world_id: String,
    pub auction_sequence: i64,
    pub lot_number: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i64>,
    pub buyer_name: String,
    pub seller_name: String,
    pub price: i64,
    pub closed_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct PriceHistoryStats {
    pub count: i64,
    pub min: i64,
    pub max: i64,
    pub median: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct PriceHistoryPage {
    pub rows: Vec<PriceHistoryEntry>,
    pub stats: Option<PriceHistoryStats>,
    pub page: Option<i64>,
    pub page_size: i64,
    pub total: Option<i64>,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub has_more: Option<bool>,
}

/// 公开价格历史的旧版筛选与分页参数。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PriceHistoryQuery {
    pub world: Option<String>,
    pub name: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub min_price: Option<String>,
    pub max_price: Option<String>,
    pub buyer: Option<String>,
    pub seller: Option<String>,
    pub page: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct PriceHistoryData {
    rows: Vec<PriceHistoryEntry>,
    stats: Option<PriceHistoryStats>,
    page: Option<i64>,
    page_size: i64,
    total: Option<i64>,
    cursor: Option<String>,
    next_cursor: Option<String>,
    has_more: Option<bool>,
}

impl From<PriceHistoryData> for PriceHistoryPage {
    fn from(data: PriceHistoryData) -> Self {
        Self {
            rows: data.rows,
            stats: data.stats,
            page: data.page,
            page_size: data.page_size,
            total: data.total,
            cursor: data.cursor,
            next_cursor: data.next_cursor,
            has_more: data.has_more,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CatalogRow {
    pub lot_id: String,
    pub code: String,
    pub name: String,
    pub category: String,
    pub status: String,
    pub seller_name: String,
    pub level: Option<i64>,
    pub avg_roll: Option<f64>,
    pub start_price: i64,
    pub current_price: i64,
    pub next_bid: i64,
    pub leader_name: Option<String>,
    pub remaining: String,
}

impl LotEnvelope {
    pub(crate) fn rows(&self) -> Vec<CatalogRow> {
        match self.lots.as_ref() {
            Some(LotItems::Preview(lots)) => lots
                .iter()
                .map(|lot| CatalogRow {
                    lot_id: lot.id.clone(),
                    code: lot.code.clone(),
                    name: lot.equipment_name.clone(),
                    category: lot.equipment_category.clone(),
                    status: lot.status.clone(),
                    seller_name: lot.seller_name.clone(),
                    level: lot.equipment_level,
                    avg_roll: lot.avg_roll,
                    start_price: lot.start_price,
                    current_price: lot.start_price,
                    next_bid: lot.start_price,
                    leader_name: None,
                    remaining: String::new(),
                })
                .collect(),
            Some(LotItems::Live(lots)) => lots
                .iter()
                .map(|lot| CatalogRow {
                    lot_id: lot.id.clone(),
                    code: lot_code_for(&lot.equipment.category, lot.lot_number),
                    name: lot.equipment.name.clone(),
                    category: lot.equipment.category.clone(),
                    status: lot.status.clone(),
                    seller_name: lot.seller_name.clone(),
                    level: lot.equipment.level,
                    avg_roll: lot.equipment.avg_roll,
                    start_price: lot.start_price,
                    current_price: lot.current_price,
                    next_bid: lot.next_bid,
                    leader_name: lot.leader_name.clone(),
                    remaining: lot.effective_end_at.clone().unwrap_or_default(),
                })
                .collect(),
            None => Vec::new(),
        }
    }
}

pub(crate) fn lot_code_for(category: &str, lot_number: i64) -> String {
    let prefix = match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => "Lot",
    };
    format!("{prefix}{lot_number:02}")
}

pub(crate) async fn fetch_worlds() -> Result<Worlds, String> {
    let response = send_get("/api/v1/worlds").await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<Worlds> = response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_current_auction(
    world_id: String,
) -> Result<Option<CurrentAuction>, String> {
    let world_query = if world_id == "main" {
        String::new()
    } else {
        let encoded = form_urlencoded::Serializer::new(String::new())
            .append_pair("world", &world_id)
            .finish();
        format!("?{encoded}")
    };
    let url = format!("/api/v1/auctions/current{world_query}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<CurrentAuctionData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.auction)
}

/// 读取指定历史拍卖的公开投影；lot 端点同时提供展示所需的拍卖上下文。
pub(crate) async fn fetch_historical_auction(auction_id: String) -> Result<CurrentAuction, String> {
    let envelope = fetch_current_lots(auction_id, None, 1).await?;
    Ok(CurrentAuction {
        id: envelope.auction_id,
        world: envelope.world,
        number: envelope.number,
        status: envelope.auction_status,
        opens_at: envelope.opens_at,
        scheduled_ends_at: envelope.scheduled_ends_at,
        presentation_mode: envelope.presentation_mode,
        bidding_open: envelope.bidding_open,
        preview_revision: envelope.preview_revision,
    })
}

pub(crate) async fn fetch_bid_log_page(
    auction_id: String,
    page: i64,
    world_id: String,
    lot_id: Option<String>,
) -> Result<BidLogPage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string()).append_pair("page_size", "50");
    if !world_id.is_empty() {
        query.append_pair("world", &world_id);
    }
    if let Some(lot_id) = lot_id {
        query.append_pair("lot", &lot_id);
    }
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<BidLogData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

/// 读取可供公开竞价日志选择的拍卖；旧版只展示 live、final 和 settled 拍卖。
pub(crate) async fn fetch_bid_log_auctions(
    world_id: String,
) -> Result<Vec<HistoricalAuction>, String> {
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
        .append_pair("page_size", "20")
        .finish();
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?{query}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<BidLogData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
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
    if let Some(world_id) = world_id.filter(|world| !world.is_empty() && world != "all") {
        query.append_pair("world", &world_id);
    }
    let url = format!("/api/v1/auctions?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AuctionListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

pub(crate) async fn fetch_auction_list() -> Result<Vec<HistoricalAuction>, String> {
    Ok(fetch_auction_page(None, "settled".to_owned(), 1, 200).await?.auctions)
}

pub(crate) async fn fetch_price_history(
    query: PriceHistoryQuery,
) -> Result<PriceHistoryPage, String> {
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
    params.append_pair("page", &query.page.max(1).to_string()).append_pair("page_size", "50");
    let url = format!("/api/v1/price-history?{}", params.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<PriceHistoryData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

pub(crate) async fn fetch_current_lots(
    auction_id: String,
    category: Option<String>,
    page: i64,
) -> Result<LotEnvelope, String> {
    let category_query = category
        .map(|value| {
            let encoded = form_urlencoded::Serializer::new(String::new())
                .append_pair("category", &value)
                .finish();
            format!("&{encoded}")
        })
        .unwrap_or_default();
    let url =
        format!("/api/v1/auctions/{auction_id}/lots?page={page}&page_size=20{category_query}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<LotEnvelope> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_lot_detail(lot_id: String) -> Result<LotEnvelope, String> {
    let url = format!("/api/v1/lots/{lot_id}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<LotEnvelope> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
