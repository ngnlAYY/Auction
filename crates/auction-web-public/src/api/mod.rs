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
    BidResult, ProxyCommandAuthorization, create_proxy_authorization, lower_proxy_authorization,
    place_bid,
};
pub(crate) use compensation::{
    CompensationMovementPage, CompensationPoolPage, CompensationRound,
    fetch_compensation_movements, fetch_compensation_pool,
};
pub(crate) use forum::{ForumThread, fetch_forum_thread};
pub(crate) use realtime::{
    HealthStatus, RealtimeEvent, RealtimePage, fetch_health, fetch_realtime_events,
    subscribe_realtime,
};
pub(crate) use rules::{RulesDocument, fetch_rules};
pub(crate) use session::{PublicSession, fetch_public_session, login_public, logout_public};

use gloo_net::http::Request;
use serde::Deserialize;

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
    pub scheduled_ends_at: Option<String>,
    pub presentation_mode: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CurrentAuctionData {
    auction: Option<CurrentAuction>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct HistoricalAuction {
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub lot_count: i64,
    pub sold_amount: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct AuctionListData {
    auctions: Vec<HistoricalAuction>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct LotEnvelope {
    pub lots: Option<LotItems>,
    pub lot: Option<LotDetailItem>,
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
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct LiveLot {
    pub id: String,
    pub lot_number: i64,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    pub effective_end_at: Option<String>,
    pub equipment: LiveEquipment,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct LiveEquipment {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum LotDetailItem {
    Preview(PreviewLot),
    Live(LiveLot),
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct BidLogEntry {
    pub id: String,
    pub lot_number: i64,
    pub amount: i64,
    pub equipment_name: String,
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
    pub status: String,
    pub price: i64,
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
                    status: lot.status.clone(),
                    price: lot.start_price,
                    remaining: String::new(),
                })
                .collect(),
            Some(LotItems::Live(lots)) => lots
                .iter()
                .map(|lot| CatalogRow {
                    lot_id: lot.id.clone(),
                    code: lot.lot_number.to_string(),
                    name: lot.equipment.name.clone(),
                    status: lot.status.clone(),
                    price: lot.current_price.max(lot.start_price),
                    remaining: lot.effective_end_at.clone().unwrap_or_default(),
                })
                .collect(),
            None => Vec::new(),
        }
    }
}

pub(crate) async fn fetch_current_auction() -> Result<Option<CurrentAuction>, String> {
    let response =
        Request::get("/api/v1/auctions/current").send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<CurrentAuctionData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.auction)
}

pub(crate) async fn fetch_current_bid_log() -> Result<Option<BidLogPage>, String> {
    let Some(auction) = fetch_current_auction().await? else {
        return Ok(None);
    };
    fetch_bid_log(auction.id).await.map(Some)
}

pub(crate) async fn fetch_bid_log(auction_id: String) -> Result<BidLogPage, String> {
    let url = format!("/api/v1/auctions/{auction_id}/bid-log?page=1&page_size=50");
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<BidLogData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

pub(crate) async fn fetch_auction_list() -> Result<Vec<HistoricalAuction>, String> {
    let response = Request::get("/api/v1/auctions?page=1&page_size=20&status=settled")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AuctionListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.auctions)
}

pub(crate) async fn fetch_price_history() -> Result<PriceHistoryPage, String> {
    let response = Request::get("/api/v1/price-history?page=1&page_size=50")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<PriceHistoryData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

pub(crate) async fn fetch_current_lots(auction_id: String) -> Result<LotEnvelope, String> {
    let url = format!("/api/v1/auctions/{auction_id}/lots?page=1&page_size=20");
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<LotEnvelope> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_lot_detail(lot_id: String) -> Result<LotEnvelope, String> {
    let url = format!("/api/v1/lots/{lot_id}");
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<LotEnvelope> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
