//! 公开只读查询端口。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::ApplicationFuture;

/// 世界目录的应用层投影，不暴露数据库记录或 SQL 类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldSnapshot {
    pub id: String,
    pub enabled: bool,
    pub availability: String,
    pub policy_revision: i64,
}

/// Public 世界目录读取端口。
pub trait WorldReader: Send + Sync {
    /// 返回当前可公开展示的世界，排序和可见性由持久化 owner 保证。
    fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>>;
}

/// 公开拍卖列表的查询条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAuctionListRequest {
    pub world_id: Option<String>,
    pub status: String,
    pub limit: i64,
    pub offset: i64,
    pub cursor: Option<PublicAuctionCursor>,
}

/// Public 拍卖列表的稳定 keyset 游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicAuctionCursor {
    Start,
    Boundary { sequence: i64, id: Uuid },
}

/// 不含内部策略、卖家身份和数据库字段的公开拍卖摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAuctionSummary {
    pub id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub scheduled_open_at: DateTime<Utc>,
    pub opened_at: Option<DateTime<Utc>>,
    pub final_phase_at: Option<DateTime<Utc>>,
    pub effective_end_at: Option<DateTime<Utc>>,
    pub settled_at: Option<DateTime<Utc>>,
    pub lot_count: i64,
    pub sold_count: i64,
    pub unsold_count: i64,
    pub sold_amount: i64,
    pub state_version: i64,
}

/// 公开拍卖列表的有界结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAuctionPage {
    pub entries: Vec<PublicAuctionSummary>,
    pub total: Option<i64>,
    pub limit: i64,
    pub offset: i64,
    pub next_cursor: Option<PublicAuctionCursor>,
    pub has_more: bool,
}

/// 当前公开 projection 的最小 HTTP 安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentAuctionSnapshot {
    pub id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub opens_at: DateTime<Utc>,
    pub scheduled_ends_at: Option<DateTime<Utc>>,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub preview_revision: i64,
}

/// Public 拍卖只读端口；具体 SQL 和后端时间/UUID 表示由存储适配器负责。
pub trait AuctionReader: Send + Sync {
    fn list_public_auctions(
        &self,
        request: PublicAuctionListRequest,
    ) -> ApplicationFuture<'_, PublicAuctionPage>;

    fn current_auction(
        &self,
        world_id: String,
    ) -> ApplicationFuture<'_, Option<CurrentAuctionSnapshot>>;
}

/// 公开 lot 读取允许的最大页大小。
pub const MAX_PUBLIC_LOT_PAGE_SIZE: i64 = 200;

/// 公开 lot 读取允许的最大 legacy offset，防止深分页拖垮数据库。
pub const MAX_PUBLIC_LOT_OFFSET: i64 = 10_000;

/// 公开 lot 分类对应的稳定 JSON key。
pub const PUBLIC_LOT_CATEGORIES: [&str; 8] =
    ["all", "one", "two", "sta", "shd", "clo", "lig", "hea"];

/// 公开 lot 列表的分类与页参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLotListRequest {
    pub category: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl PublicLotListRequest {
    /// 在应用端口边界拒绝未定义分类、无界页大小和深 offset。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.limit < 1 || self.limit > MAX_PUBLIC_LOT_PAGE_SIZE || self.offset < 0 {
            return Err("公开 lot 分页参数无效");
        }
        if self.offset > MAX_PUBLIC_LOT_OFFSET {
            return Err("公开 lot 分页 offset 超出上限");
        }
        if self
            .category
            .as_deref()
            .is_some_and(|category| !PUBLIC_LOT_CATEGORIES[1..].contains(&category))
        {
            return Err("公开 lot 分类参数无效");
        }
        Ok(())
    }
}

/// Public lot 列表与详情共用的公开拍卖投影。
#[derive(Debug, Clone)]
pub struct PublicAuctionProjection {
    pub auction_id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub auction_status: String,
    pub projection_kind: String,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub opens_at: DateTime<Utc>,
    pub scheduled_ends_at: Option<DateTime<Utc>>,
    pub preview_revision: i64,
    pub final_restriction: Option<PublicFinalRestriction>,
}

/// 终阶段对代理操作的公开限制说明。
#[derive(Debug, Clone, Copy)]
pub struct PublicFinalRestriction {
    pub new_slow_proxy_allowed: bool,
    pub existing_slow_proxy_converted: bool,
}

/// 公开装备 stat 的渲染字段。
#[derive(Debug, Clone)]
pub struct PublicEquipmentStat {
    pub value: String,
    pub base: Option<f64>,
    pub roll: Option<f64>,
}

/// 不包含 key、邮件和托管信息的公开装备投影。
#[derive(Debug, Clone)]
pub struct PublicEquipment {
    pub name: String,
    pub category: String,
    pub level: Option<i64>,
    pub info_summary: String,
    pub stats: BTreeMap<String, PublicEquipmentStat>,
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
    pub attributes: Value,
}

/// 正式竞拍 lot 的公开摘要。
#[derive(Debug, Clone)]
pub struct PublicLiveLotSummary {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub lot_number: i64,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub current_price: i64,
    pub next_bid: i64,
    pub has_leader: bool,
    pub leader_name: Option<String>,
    pub effective_end_at: Option<DateTime<Utc>>,
    pub state_version: i64,
    pub equipment: PublicEquipment,
}

/// 正式竞拍 lot 详情；attributes 只在详情分支通过 HTTP 映射公开。
#[derive(Debug, Clone)]
pub struct PublicLiveLotDetail {
    pub summary: PublicLiveLotSummary,
    pub equipment_attributes: Value,
}

/// 预览 lot 的公开摘要。
#[derive(Debug, Clone)]
pub struct PublicPreviewLotSummary {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub code: String,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i64>,
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
}

/// 预览 lot 详情。
#[derive(Debug, Clone)]
pub struct PublicPreviewLotDetail {
    pub summary: PublicPreviewLotSummary,
    pub equipment: PublicEquipment,
}

/// 当前公开 projection 下的单个 lot 详情。
#[derive(Debug, Clone)]
pub struct PublicLotDetail {
    pub projection: PublicAuctionProjection,
    pub preview: Option<PublicPreviewLotDetail>,
    pub live: Option<PublicLiveLotDetail>,
}

/// 当前公开 projection 下的 lot 分页。
#[derive(Debug, Clone)]
pub struct PublicLotPage {
    pub projection: PublicAuctionProjection,
    pub preview: Vec<PublicPreviewLotSummary>,
    pub live: Vec<PublicLiveLotSummary>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub category_counts: BTreeMap<String, i64>,
}

/// Public lot 只读端口；未出现在当前公开 projection 的 lot 必须返回 None。
pub trait PublicLotReader: Send + Sync {
    fn list_public_lots(
        &self,
        auction_id: Uuid,
        request: PublicLotListRequest,
    ) -> ApplicationFuture<'_, Option<PublicLotPage>>;

    fn public_lot(&self, lot_id: Uuid) -> ApplicationFuture<'_, Option<PublicLotDetail>>;
}

/// 账户竞价读取允许的最大页大小。
pub const MAX_PUBLIC_ACCOUNT_PAGE_SIZE: i64 = 200;

/// 账户竞价兼容页允许的最大 offset。
pub const MAX_PUBLIC_ACCOUNT_OFFSET: i64 = 10_000;

/// 旧账户接口的默认页大小。
pub const DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE: i64 = 20;

/// 账户竞价与代理历史的一张数字页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicAccountPageRequest {
    pub limit: i64,
    pub offset: i64,
}

impl PublicAccountPageRequest {
    /// 拒绝无界页大小、负 offset 和过深的兼容分页。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.limit < 1 || self.limit > MAX_PUBLIC_ACCOUNT_PAGE_SIZE || self.offset < 0 {
            return Err("公开账户分页参数无效");
        }
        if self.offset > MAX_PUBLIC_ACCOUNT_OFFSET {
            return Err("公开账户分页 offset 超出上限");
        }
        Ok(())
    }
}

/// 当前用户竞价和代理历史的独立分页请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicMyBidsRequest {
    pub bids: PublicAccountPageRequest,
    pub proxies: PublicAccountPageRequest,
}

impl PublicMyBidsRequest {
    /// 在应用端口边界同时校验两张账户表的分页。
    pub fn validate(&self) -> Result<(), &'static str> {
        self.bids.validate()?;
        self.proxies.validate()
    }
}

/// 当前用户自己的竞价事实，并附带最小装备公开投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicMyBid {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub lot_number: i64,
    pub amount: i64,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: DateTime<Utc>,
    pub equipment_name: String,
    pub equipment_category: String,
    pub leading: bool,
}

/// 代理产生的最近一次公开竞价，不携带代理上限等 owner-only 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicProxyBid {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub lot_number: i64,
    pub amount: i64,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: DateTime<Utc>,
    pub leading: bool,
}

/// 代理授权历史中的一条 append-only 转换事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicProxyTransition {
    pub transition_number: i64,
    pub kind: String,
    pub old_maximum_amount: Option<i64>,
    pub new_maximum_amount: i64,
    pub old_mode: Option<String>,
    pub new_mode: String,
    pub old_status: Option<String>,
    pub new_status: String,
    pub reason: Option<String>,
    pub resulting_state_version: i64,
    pub created_at: DateTime<Utc>,
}

/// owner-only 代理授权的派生操作能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicProxyCapabilities {
    pub can_lower: bool,
    pub waiting: bool,
    pub at_maximum: bool,
}

/// 当前用户在指定公开 lot 上拥有的代理授权投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicOwnedProxy {
    pub authorization_id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub user_id: Uuid,
    pub lot_number: i64,
    pub authorization_sequence: i64,
    pub usage_number: i64,
    pub quota_used: i64,
    pub quota_limit: i64,
    pub mode: String,
    pub maximum_amount: i64,
    pub status: String,
    pub terminal_reason: Option<String>,
    pub state_version: i64,
    pub transitions: Vec<PublicProxyTransition>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub equipment_name: String,
    pub equipment_category: String,
    pub bid: Option<PublicProxyBid>,
    pub invalid_reason: Option<String>,
    pub capabilities: PublicProxyCapabilities,
}

/// 当前用户竞价和代理历史的结果页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicMyBidsPage {
    pub bids: Vec<PublicMyBid>,
    pub proxies: Vec<PublicOwnedProxy>,
    pub bid_total: i64,
    pub proxy_total: i64,
}

/// Public 当前用户账户读取端口。
pub trait PublicAccountReader: Send + Sync {
    fn list_public_user_bids(
        &self,
        user_id: Uuid,
        request: PublicMyBidsRequest,
    ) -> ApplicationFuture<'_, PublicMyBidsPage>;

    fn public_user_proxy(
        &self,
        lot_id: Uuid,
        user_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicOwnedProxy>>;

    /// 按授权身份读取历史代理投影，供命令重放时避免被后续重入授权遮蔽。
    fn public_user_proxy_by_id(
        &self,
        lot_id: Uuid,
        authorization_id: Uuid,
        user_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicOwnedProxy>>;
}

/// 公开竞价日志允许的最大页大小。
pub const MAX_PUBLIC_BID_LOG_PAGE_SIZE: i64 = 200;

/// 公开竞价日志兼容页允许的最大 offset。
pub const MAX_PUBLIC_BID_LOG_OFFSET: i64 = 10_000;

/// 单场公开竞价日志的筛选与分页条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBidLogListRequest {
    pub auction_id: Uuid,
    pub world_id: Option<String>,
    pub lot_id: Option<Uuid>,
    pub limit: i64,
    pub offset: i64,
    pub cursor: Option<PublicBidLogCursor>,
}

impl PublicBidLogListRequest {
    /// 在应用端口边界校验资源标识、筛选文本和两种分页模式的互斥关系。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_nil()
            || self
                .world_id
                .as_deref()
                .is_some_and(|world| world.is_empty() || world.len() > 64 || world != world.trim())
            || self.lot_id.is_some_and(|lot_id| lot_id.is_nil())
            || self.limit < 1
            || self.limit > MAX_PUBLIC_BID_LOG_PAGE_SIZE
            || self.offset < 0
            || self.offset > MAX_PUBLIC_BID_LOG_OFFSET
            || self.cursor.is_some() && self.offset != 0
        {
            return Err("公开竞价日志分页参数无效");
        }
        if let Some(PublicBidLogCursor::Boundary {
            created_at,
            lot_id,
            accepted_state_version,
            fact_index,
            id,
        }) = self.cursor.as_ref()
            && (created_at.timestamp_micros() == 0
                || lot_id.is_nil()
                || *accepted_state_version <= 0
                || *fact_index <= 0
                || id.is_nil())
        {
            return Err("公开竞价日志游标无效");
        }
        Ok(())
    }
}

/// 公开竞价日志的完整降序 keyset 边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicBidLogCursor {
    Start,
    Boundary {
        created_at: DateTime<Utc>,
        lot_id: Uuid,
        accepted_state_version: i64,
        fact_index: i32,
        id: Uuid,
    },
}

/// 公开竞价日志条目；不包含代理上限、Cookie 或其他私有竞价状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBidLogEntry {
    pub id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub lot_number: i64,
    pub amount: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub bidder_name: String,
    pub valid: bool,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: DateTime<Utc>,
}

/// 公开竞价日志分页结果；cursor 页不计算全量 total。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBidLogPage {
    pub entries: Vec<PublicBidLogEntry>,
    pub total: Option<i64>,
    pub limit: i64,
    pub offset: i64,
    pub next_cursor: Option<PublicBidLogCursor>,
    pub has_more: bool,
}

/// Public 竞价日志只读端口。
pub trait PublicBidLogReader: Send + Sync {
    fn list_public_bid_log(
        &self,
        request: PublicBidLogListRequest,
    ) -> ApplicationFuture<'_, Option<PublicBidLogPage>>;
}

/// 公开价格历史允许的最大名称词条数。
pub const MAX_PUBLIC_HISTORY_NAME_TERMS: usize = 8;

/// 公开价格历史允许的最大页大小。
pub const MAX_PUBLIC_PRICE_HISTORY_PAGE_SIZE: i64 = 200;

/// 公开价格历史兼容页允许的最大 offset。
pub const MAX_PUBLIC_PRICE_HISTORY_OFFSET: i64 = 10_000;

/// 公开价格历史的筛选与分页条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPriceHistoryListRequest {
    pub name_terms: Vec<String>,
    pub world_id: Option<String>,
    pub closed_from: Option<DateTime<Utc>>,
    pub closed_before: Option<DateTime<Utc>>,
    pub minimum_price: Option<i64>,
    pub maximum_price: Option<i64>,
    pub buyer: Option<String>,
    pub seller: Option<String>,
    pub limit: i64,
    pub offset: i64,
    pub cursor: Option<PublicPriceHistoryCursor>,
}

impl PublicPriceHistoryListRequest {
    /// 校验旧价格历史过滤器、半开时间区间和编号/cursor 分页边界。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name_terms.len() > MAX_PUBLIC_HISTORY_NAME_TERMS
            || self
                .name_terms
                .iter()
                .any(|term| term.is_empty() || term.len() > 128 || term != term.trim())
            || self
                .world_id
                .as_deref()
                .is_some_and(|world| world.is_empty() || world.len() > 64 || world != world.trim())
            || self.buyer.as_deref().is_some_and(|buyer| buyer.is_empty() || buyer.len() > 256)
            || self.seller.as_deref().is_some_and(|seller| seller.is_empty() || seller.len() > 256)
            || self.closed_from.is_some_and(|value| value.timestamp_micros() == 0)
            || self.closed_before.is_some_and(|value| value.timestamp_micros() == 0)
            || self.closed_from.zip(self.closed_before).is_some_and(|(from, before)| from >= before)
            || self.minimum_price.is_some_and(|price| price < 0)
            || self.maximum_price.is_some_and(|price| price < 0)
            || self
                .minimum_price
                .zip(self.maximum_price)
                .is_some_and(|(minimum, maximum)| minimum > maximum)
            || self.limit < 1
            || self.limit > MAX_PUBLIC_PRICE_HISTORY_PAGE_SIZE
            || self.offset < 0
            || self.offset > MAX_PUBLIC_PRICE_HISTORY_OFFSET
            || self.cursor.is_some() && self.offset != 0
        {
            return Err("公开价格历史参数无效");
        }
        if self.name_terms.iter().any(|term| term.chars().any(char::is_control))
            || self.buyer.as_deref().is_some_and(|buyer| buyer.chars().any(char::is_control))
            || self.seller.as_deref().is_some_and(|seller| seller.chars().any(char::is_control))
        {
            return Err("公开价格历史文本参数无效");
        }
        if let Some(PublicPriceHistoryCursor::Boundary {
            closed_at,
            auction_sequence,
            lot_number,
            lot_id,
        }) = self.cursor.as_ref()
            && (closed_at.timestamp_micros() == 0
                || *auction_sequence <= 0
                || *lot_number <= 0
                || lot_id.is_nil())
        {
            return Err("公开价格历史游标无效");
        }
        Ok(())
    }
}

/// 公开价格历史的混合降序/升序 keyset 边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicPriceHistoryCursor {
    Start,
    Boundary { closed_at: DateTime<Utc>, auction_sequence: i64, lot_number: i64, lot_id: Uuid },
}

/// 已结算 sold lot 的公开成交事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPriceHistoryEntry {
    pub lot_id: Uuid,
    pub auction_id: Uuid,
    pub world_id: String,
    pub auction_sequence: i64,
    pub lot_number: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i64>,
    pub buyer_name: String,
    pub seller_name: String,
    pub price: i64,
    pub closed_at: DateTime<Utc>,
}

/// 编号页返回的完整过滤集价格统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicPriceHistoryStats {
    pub count: i64,
    pub minimum: i64,
    pub maximum: i64,
    pub median: i64,
}

/// 公开价格历史页；cursor 页不计算全量统计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPriceHistoryPage {
    pub entries: Vec<PublicPriceHistoryEntry>,
    pub total: Option<i64>,
    pub stats: Option<PublicPriceHistoryStats>,
    pub limit: i64,
    pub offset: i64,
    pub next_cursor: Option<PublicPriceHistoryCursor>,
    pub has_more: bool,
}

/// Public 价格历史只读端口。
pub trait PublicPriceHistoryReader: Send + Sync {
    fn list_public_price_history(
        &self,
        request: PublicPriceHistoryListRequest,
    ) -> ApplicationFuture<'_, PublicPriceHistoryPage>;
}
