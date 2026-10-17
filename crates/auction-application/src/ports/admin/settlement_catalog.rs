//! Admin 结算报表选择目录的窄只读端口。

use chrono::{DateTime, Utc};

use auction_domain::AuctionId;

use super::ApplicationFuture;

pub const MAX_SETTLEMENT_CATALOG_PAGE_SIZE: i64 = 100;
pub const MAX_SETTLEMENT_CATALOG_OFFSET: i64 = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementCatalogRequest {
    pub world_id: Option<String>,
    pub sequence: Option<i64>,
    pub limit: i64,
    pub offset: i64,
}

impl SettlementCatalogRequest {
    pub fn validate(&self) -> bool {
        self.world_id.as_deref().is_none_or(|world| matches!(world, "main" | "isekai"))
            && self.sequence.is_none_or(|sequence| sequence > 0)
            && self.limit > 0
            && self.limit <= MAX_SETTLEMENT_CATALOG_PAGE_SIZE
            && self.offset >= 0
            && self.offset <= MAX_SETTLEMENT_CATALOG_OFFSET
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementCatalogEntry {
    pub auction_id: AuctionId,
    pub world_id: String,
    pub sequence: i64,
    pub settled_at: DateTime<Utc>,
}

impl SettlementCatalogEntry {
    pub fn validate(&self) -> bool {
        !self.auction_id.is_nil()
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && self.sequence > 0
            && self.settled_at.timestamp_micros() != 0
    }
}

/// 已结算拍卖目录页；单世界按期号降序，全世界按结算时间降序，均由存储 owner 稳定排序。
#[derive(Debug, Clone)]
pub struct SettlementCatalogPage {
    pub entries: Vec<SettlementCatalogEntry>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl SettlementCatalogPage {
    pub fn validate(&self, request: &SettlementCatalogRequest) -> bool {
        request.validate()
            && self.total >= 0
            && self.limit == request.limit
            && self.offset == request.offset
            && i64::try_from(self.entries.len()).is_ok_and(|count| count <= self.limit)
            && self.entries.iter().all(|entry| {
                entry.validate()
                    && request.world_id.as_deref().is_none_or(|world| entry.world_id == world)
                    && request.sequence.is_none_or(|sequence| entry.sequence == sequence)
            })
    }
}

pub trait SettlementCatalogReader: Send + Sync {
    fn list_settlement_catalog(
        &self,
        request: SettlementCatalogRequest,
    ) -> ApplicationFuture<'_, SettlementCatalogPage>;
}
