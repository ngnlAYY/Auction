//! settlement 相关 DTO。

use super::*;

/// Admin 结算报告中的拍卖生命周期投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementAuctionResponse {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub preview_scheduled_at: String,
    pub scheduled_open_at: String,
    pub preview_opened_at: Option<String>,
    pub preview_manual_open: bool,
    pub preview_revision: i64,
    pub opened_at: Option<String>,
    pub final_phase_at: Option<String>,
    pub settled_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub cancelled_reason: Option<String>,
    pub cancelled_by: Option<String>,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Admin 拍卖列表的能力声明。
///
/// 能力只表示对应命令路由已经存在；未声明的扩展命令仍保持明确的未迁移响应，不会由能力
/// 字段推断出额外的业务 owner。
///
/// `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐：wire 冻结后服务端字段集
/// 不变，收紧只影响反序列化，不影响序列化输出。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionCapabilitiesResponse {
    pub cancel_auction: bool,
    pub manual_early_open: bool,
    pub manual_early_settlement: bool,
    pub manual_intake: bool,
    pub manual_preview_open: bool,
    pub modify_schedule: bool,
}

/// Admin 拍卖列表中的生命周期安全投影。
///
/// 反序列化按旧浏览器 decoder 的闭集语义收紧：nullable 生命周期字段必须显式出现（可为
/// `null`），`lot_count` 允许省略但出现时不得为 `null`；未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionResponse {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub preview_scheduled_at: String,
    pub scheduled_open_at: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub preview_opened_at: Option<String>,
    pub preview_manual_open: bool,
    pub preview_revision: i64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub opened_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub final_phase_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub settled_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub cancelled_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub cancelled_reason: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub cancelled_by: Option<String>,
    pub state_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_ends_at: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub lot_count: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// `Option` 默认会把缺失字段折叠成 `None`；旧 v1 decoder 要求 nullable 字段必须显式出现。
fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// 旧 wire 的兼容扩展字段：允许缺失，但一旦出现就不接受 JSON `null`。
fn deserialize_optional_field<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Admin 拍卖列表分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 拍卖列表响应；列表和 lot 数量来自同一 PostgreSQL 快照。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionListResponse {
    pub auctions: Vec<AdminAuctionResponse>,
    pub capabilities: AdminAuctionCapabilitiesResponse,
    pub pagination: AdminAuctionPaginationResponse,
}

/// Admin 结算摘要响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementSummaryResponse {
    pub opening_balance: i64,
    pub tax_income: i64,
    pub discount_demand: i64,
    pub compensation_out: i64,
    pub closing_balance: i64,
    pub movement_count: i64,
    pub settled_at: String,
}

/// Admin 结算 lot 的 adjustment metadata。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementAdjustmentResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
}

/// Admin 结算报告中的单个 lot。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementLotResponse {
    pub id: String,
    pub lot_number: i32,
    pub status: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub seller: Option<String>,
    pub winner: Option<String>,
    pub final_price: i64,
    pub listed_price: i64,
    pub buyer_amount: i64,
    pub seller_amount: i64,
    pub tax_amount: i64,
    pub compensation_amount: i64,
    pub hold_ppm: i32,
    pub adjustment_ppm: i32,
    pub adjustment: AdminSettlementAdjustmentResponse,
}

/// Admin 结算报告响应；未结算拍卖的 `settlement` 为 null，且不伪造 lot 金额。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminSettlementReportResponse {
    pub auction: AdminSettlementAuctionResponse,
    pub settlement: Option<AdminSettlementSummaryResponse>,
    pub lots: Vec<AdminSettlementLotResponse>,
}
