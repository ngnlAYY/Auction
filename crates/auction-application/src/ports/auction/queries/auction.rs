//! Public 拍卖目录和当前拍卖读取端口。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ApplicationFuture;

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
