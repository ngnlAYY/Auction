//! lot 相关 DTO。

use super::*;

/// Admin lot 的安全投影；不包含装备密钥、代理上限或原始外部正文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotResponse {
    pub id: String,
    pub auction_id: String,
    pub equipment_id: String,
    pub submission_id: Option<String>,
    pub lot_number: i32,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    pub leader_user_id: Option<String>,
    pub effective_end_at: Option<String>,
    pub slow_due_at: Option<String>,
    pub slow_generation: i64,
    pub slow_authorization_id: Option<String>,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_attributes: Value,
    pub equipment_collected_at: String,
    pub seller: Option<String>,
    pub submission_source_kind: Option<String>,
    pub submission_source_reference: Option<String>,
    pub mailbox_key: Option<String>,
    pub external_mail_id: Option<String>,
    pub submission_received_at: Option<String>,
}

/// Admin draft lot 修改成功响应；返回同一事务快照中的 lot 安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotUpdateResponse {
    pub lot: AdminLotResponse,
}

/// Admin lot 列表分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin lot 页面可用的外部写入能力；浏览器不得根据数据库档位自行猜测。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotCapabilitiesResponse {
    pub manual_intake: bool,
}

/// Admin lot 列表响应；拍卖和 lot 来自同一数据库快照。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminLotListResponse {
    pub auction: AdminSettlementAuctionResponse,
    pub lots: Vec<AdminLotResponse>,
    pub pagination: AdminLotPaginationResponse,
    pub capabilities: AdminLotCapabilitiesResponse,
}
