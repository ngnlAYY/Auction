//! lot 相关 DTO。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::response::deserialize_required_option;
use super::AdminAuctionResponse;

/// Admin lot 的安全投影；不包含装备密钥、代理上限或原始外部正文。
/// 反序列化按旧浏览器 decoder 的闭集语义收紧：nullable 字段必须显式出现（可为 `null`），
/// 未知字段一律拒绝；wire 冻结后服务端字段集不变，序列化输出不受影响。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminLotResponse {
    pub id: String,
    pub auction_id: String,
    pub equipment_id: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub submission_id: Option<String>,
    pub lot_number: i32,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub leader_user_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub effective_end_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub slow_due_at: Option<String>,
    pub slow_generation: i64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub slow_authorization_id: Option<String>,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_attributes: Value,
    pub equipment_collected_at: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub seller: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub submission_source_kind: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub submission_source_reference: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub mailbox_key: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub external_mail_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub submission_received_at: Option<String>,
}

/// Admin draft lot 修改成功响应；返回同一事务快照中的 lot 安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminLotUpdateResponse {
    pub lot: AdminLotResponse,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminLotPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin lot 列表响应；拍卖和 lot 来自同一数据库快照。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminLotListResponse {
    pub auction: AdminAuctionResponse,
    pub lots: Vec<AdminLotResponse>,
    pub pagination: AdminLotPaginationResponse,
}
