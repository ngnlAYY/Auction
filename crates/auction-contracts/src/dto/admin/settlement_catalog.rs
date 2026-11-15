//! Admin 结算报表选择目录 DTO。

use serde::{Deserialize, Serialize};

use super::pagination::AdminStrictPaginationResponse;

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementCatalogEntryResponse {
    pub auction_id: String,
    pub world_id: String,
    pub sequence: i64,
    pub settled_at: String,
}

/// 结算报表选择目录的数字分页。
pub type AdminSettlementCatalogPaginationResponse = AdminStrictPaginationResponse;

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementCatalogResponse {
    pub auctions: Vec<AdminSettlementCatalogEntryResponse>,
    pub pagination: AdminSettlementCatalogPaginationResponse,
}
