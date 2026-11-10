//! Admin 结算报表选择目录 DTO。

use super::*;

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementCatalogEntryResponse {
    pub auction_id: String,
    pub world_id: String,
    pub sequence: i64,
    pub settled_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementCatalogPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSettlementCatalogResponse {
    pub auctions: Vec<AdminSettlementCatalogEntryResponse>,
    pub pagination: AdminSettlementCatalogPaginationResponse,
}
