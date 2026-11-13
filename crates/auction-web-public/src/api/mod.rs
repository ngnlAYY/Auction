//! Public 浏览器端的同源 HTTP 数据访问。
//! 这里仅解码 HTTP 安全投影，不引入数据库模型或应用层端口。请求失败会作为页面状态返回，
//! 组件可以显示可本地化的加载/错误文案，而不是继续渲染未经权威 API 证实的数据。所有 wire DTO
//! 统一由 `auction-contracts` 拥有，本 crate 只保留浏览器展示与查询状态。

mod account;
mod blacklist;
mod clock;
mod commands;
mod compensation;
mod decoder;
mod forum;
mod model;
mod queries;
mod realtime;
mod registration;
mod rules;
mod session;
mod session_preference;
mod status;
mod swr_cache;
mod watch_preference;
mod watches;

pub(crate) use session_preference::{
    fetch_session_preference, set_session_preference, valid_session_duration,
};
pub(crate) use watch_preference::{
    channel_flags, channel_value, fetch_lot_watch_preference, set_lot_watch_preference,
};

#[cfg(target_arch = "wasm32")]
pub(crate) use watches::valid_watch_collection;
pub(crate) use watches::{fetch_lot_watches, set_lot_watch};

pub(crate) use account::{MyBidsPage, OwnedBid, OwnedProxy, fetch_my_bids, fetch_my_proxy};
pub(crate) use blacklist::{BlacklistEntry, BlacklistPage, fetch_blacklist};
pub(crate) use clock::{estimated_now, send_get, send_get_conditional, send_request};
// 传输边界（10 秒取消上限、结构化 TransportError）由 auction-web-shared 统一提供。
pub(crate) use auction_web_shared::TransportError;
pub(crate) use commands::{
    CommandError, ProxyCommandAuthorization, create_proxy_authorization, lower_proxy_authorization,
    place_bid,
};
pub(crate) use compensation::{
    CompensationLot, CompensationLotPage, CompensationPoolPage, CompensationRound,
    fetch_compensation_lots, fetch_compensation_pool,
};
pub(crate) use forum::{ForumThread, fetch_forum_sync_enabled, fetch_forum_thread};
pub(crate) use model::{
    CatalogLevelFilter, CatalogRow, LotEnvelopeRows, PriceHistoryQuery, catalog_row_from_detail,
    lot_code_for,
};
pub(crate) use queries::{
    LotDetailFetchError, PriceHistoryFetchError, fetch_auction_history_page, fetch_auction_list,
    fetch_bid_log_auctions, fetch_bid_log_page, fetch_current_auction, fetch_current_lots,
    fetch_historical_auction, fetch_lot_bid_log, fetch_lot_detail, fetch_price_history,
    fetch_worlds,
};
pub(crate) use realtime::{
    RealtimeCache, RealtimeEvent, RealtimePage, subscribe_auction_realtime, subscribe_realtime,
};
pub(crate) use registration::fetch_registration_info;
pub(crate) use rules::{DOCUMENT_UNPUBLISHED, RulesDocument, SiteDocument, fetch_site_document};
#[cfg(target_arch = "wasm32")]
pub(crate) use session::PUBLIC_SESSION_CHANGE_STORAGE_KEY;
pub(crate) use session::{
    PublicSession, PublicSessionApiError, PublicSessionCheckTrigger, SessionRefreshReason,
    fetch_public_session, login_public, logout_public,
};
pub(crate) use status::fetch_public_readiness;
// 公开读缓存只在实时事件与成功命令后整体失效；查询与测试直接使用模块内核心。
pub(crate) use swr_cache::invalidate_all;

use auction_contracts::ApiResponse;
pub(crate) use auction_contracts::{
    AuctionListResponse as AuctionListPage, AuctionSummaryResponse as HistoricalAuction,
    AuctionSummaryV2Response as HistoricalAuctionWithForum,
    CurrentAuctionItemResponse as CurrentAuction, PublicBidLogEntryResponse as BidLogEntry,
    PublicBidLogListResponse as BidLogPage, PublicEquipmentResponse as EquipmentDetail,
    PublicLiveLotResponse as LiveLot, PublicLotDetailItemResponse as LotDetailItem,
    PublicLotEnvelopeResponse as LotEnvelope, PublicLotListItemsResponse as LotItems,
    PublicPreviewEquipmentResponse as PreviewEquipmentDetail,
    PublicPreviewLotDetailResponse as PreviewLotDetail,
    PublicPriceHistoryEntryResponse as PriceHistoryEntry,
    PublicPriceHistoryListResponse as PriceHistoryPage,
    PublicPriceHistoryStatsResponse as PriceHistoryStats, WorldStatusResponse as WorldStatus,
    WorldsResponse as Worlds,
};
