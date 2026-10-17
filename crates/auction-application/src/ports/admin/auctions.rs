//! Admin 拍卖列表读取端口。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{ApplicationFuture, SettlementAuctionSnapshot};

/// Admin 拍卖列表允许的最大单页大小。
pub const MAX_ADMIN_AUCTION_PAGE_SIZE: i64 = 100;

/// Admin 拍卖列表允许的最大 offset。
pub const MAX_ADMIN_AUCTION_OFFSET: i64 = 100_000;

/// Admin 拍卖列表允许的状态闭集。
pub const ADMIN_AUCTION_STATUSES: [&str; 7] =
    ["draft", "preview", "live", "final", "settling", "settled", "cancelled"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminAuctionListRequest {
    pub world_id: Option<String>,
    pub status: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl AdminAuctionListRequest {
    /// 校验筛选器和有界分页。
    pub fn validate(&self) -> bool {
        valid_optional_text(self.world_id.as_deref(), 64)
            && self.status.as_deref().is_none_or(|status| ADMIN_AUCTION_STATUSES.contains(&status))
            && self.limit > 0
            && self.limit <= MAX_ADMIN_AUCTION_PAGE_SIZE
            && self.offset >= 0
            && self.offset <= MAX_ADMIN_AUCTION_OFFSET
    }
}

/// Admin 拍卖列表中的生命周期快照和 lot 数量。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminAuctionSummary {
    pub auction: SettlementAuctionSnapshot,
    pub scheduled_ends_at: Option<DateTime<Utc>>,
    pub lot_count: i64,
}

impl AdminAuctionSummary {
    /// 校验列表条目的安全生命周期字段。
    pub fn validate(&self) -> bool {
        let auction = &self.auction;
        !auction.id.is_nil()
            && valid_text(&auction.world_id, 64)
            && ADMIN_AUCTION_STATUSES.contains(&auction.status.as_str())
            && auction.sequence > 0
            && valid_time(auction.preview_scheduled_at)
            && valid_time(auction.scheduled_open_at)
            && auction.preview_scheduled_at <= auction.scheduled_open_at
            && valid_optional_time(auction.preview_opened_at)
            && (!auction.preview_manual_open || auction.preview_opened_at.is_some())
            && auction.opened_at.is_none_or(|opened| {
                auction.preview_opened_at.is_some_and(|preview_opened| opened >= preview_opened)
            })
            && auction.preview_revision >= 0
            && valid_optional_time(auction.opened_at)
            && valid_optional_time(auction.final_phase_at)
            && auction.final_phase_at.is_none_or(|final_phase| {
                auction.opened_at.is_some_and(|opened| final_phase >= opened)
            })
            && valid_optional_time(auction.settled_at)
            && valid_optional_time(auction.cancelled_at)
            && auction.state_version > 0
            && valid_optional_time(auction.scheduled_end_at)
            && valid_optional_time(auction.effective_end_at)
            && valid_time(auction.created_at)
            && valid_time(auction.updated_at)
            && auction.created_at <= auction.updated_at
            && valid_cancellation(auction)
            && valid_optional_time(self.scheduled_ends_at)
            && self.lot_count >= 0
    }
}

#[derive(Debug, Clone)]
pub struct AdminAuctionPage {
    pub entries: Vec<AdminAuctionSummary>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl AdminAuctionPage {
    /// 校验分页和每个生命周期快照。
    pub fn validate(&self, request: &AdminAuctionListRequest) -> bool {
        request.validate()
            && self.total >= 0
            && self.limit == request.limit
            && self.offset == request.offset
            && i64::try_from(self.entries.len()).is_ok_and(|entry_count| entry_count <= self.limit)
            && self.entries.iter().all(AdminAuctionSummary::validate)
    }
}

pub trait AdminAuctionReader: Send + Sync {
    fn list_admin_auctions(
        &self,
        request: AdminAuctionListRequest,
    ) -> ApplicationFuture<'_, AdminAuctionPage>;
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| valid_text(value, maximum))
}

fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_micros() != 0
}

fn valid_optional_time(value: Option<DateTime<Utc>>) -> bool {
    value.is_none_or(valid_time)
}

fn valid_cancellation(auction: &SettlementAuctionSnapshot) -> bool {
    match auction.status.as_str() {
        "cancelled" => {
            auction.cancelled_at.is_some()
                && valid_optional_text(auction.cancelled_by.as_deref(), 128)
                && valid_optional_text(auction.cancelled_reason.as_deref(), 2048)
        }
        _ => {
            auction.cancelled_at.is_none()
                && auction.cancelled_by.is_none()
                && auction.cancelled_reason.is_none()
        }
    }
}
