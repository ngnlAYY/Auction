//! Public 正式/预览 lot、装备和响应外壳 DTO。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 终阶段代理限制的公开契约。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicFinalRestrictionResponse {
    pub new_slow_proxy_allowed: bool,
    pub existing_slow_proxy_converted: bool,
}

/// 公开装备 stat 的稳定 JSON 形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicEquipmentStatResponse {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
}

/// 公开装备详情；attributes 仅由 lot 详情显式传入。
/// 完整游戏装备 URL 含 key，允许 wire 展示但不能进入调试输出。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicEquipmentResponse {
    pub name: String,
    pub category: String,
    /// 与论坛标色规则同源的装备分组：`rare` 或 `fusion`。
    pub equipment_group: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_nonempty_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub primary_attribute_initials: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub info_summary: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stats: BTreeMap<String, PublicEquipmentStatResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_roll: Option<f64>,
    /// P09/P11 返回含 10 位 key 的真实 URL；历史空值只作兼容，不证明链接可用。
    pub game_equipment_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<serde_json::Value>,
}

impl std::fmt::Debug for PublicEquipmentResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicEquipmentResponse")
            .field("name", &self.name)
            .field("category", &self.category)
            .field("equipment_group", &self.equipment_group)
            .field("level", &self.level)
            .field("primary_attribute_initials", &self.primary_attribute_initials)
            .field("info_summary", &self.info_summary)
            .field("stats", &self.stats)
            .field("avg_roll", &self.avg_roll)
            .field("game_equipment_url", &"[REDACTED]")
            .field("attributes", &self.attributes)
            .finish()
    }
}

/// 正式竞拍 lot 的公开契约。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLiveLotResponse {
    pub id: String,
    pub auction_id: String,
    pub lot_number: i64,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    pub next_bid: i64,
    pub has_leader: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader_name: Option<String>,
    pub seller_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_end_at: Option<String>,
    pub state_version: i64,
    pub equipment: PublicEquipmentResponse,
}

/// 预览 lot 的公开契约。
/// 调试输出保留公开装备属性，但隐藏完整游戏装备 URL 中的 key。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPreviewLotResponse {
    pub id: String,
    pub auction_id: String,
    pub code: String,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub equipment_name: String,
    /// 与正式目录共用的装备分组：`rare` 或 `fusion`。
    pub equipment_group: String,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_nonempty_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub equipment_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_level: Option<i64>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_nonempty_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub primary_attribute_initials: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_roll: Option<f64>,
    /// P09/P11 返回含 10 位 key 的真实 URL；历史空值只作兼容，不证明链接可用。
    pub game_equipment_url: String,
}

impl std::fmt::Debug for PublicPreviewLotResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicPreviewLotResponse")
            .field("id", &self.id)
            .field("auction_id", &self.auction_id)
            .field("code", &self.code)
            .field("status", &self.status)
            .field("seller_name", &self.seller_name)
            .field("start_price", &self.start_price)
            .field("equipment_name", &self.equipment_name)
            .field("equipment_group", &self.equipment_group)
            .field("equipment_category", &self.equipment_category)
            .field("equipment_level", &self.equipment_level)
            .field("primary_attribute_initials", &self.primary_attribute_initials)
            .field("avg_roll", &self.avg_roll)
            .field("game_equipment_url", &"[REDACTED]")
            .finish()
    }
}

/// 预览详情装备的公开契约；分类在旧版 wire 中允许缺失。
/// 与正式详情相同，完整游戏装备 URL 只进入 wire，不进入调试输出。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPreviewEquipmentResponse {
    pub name: String,
    /// 必须与预览摘要的装备分组相同。
    pub equipment_group: String,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_nonempty_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_nonempty_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub primary_attribute_initials: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub info_summary: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stats: BTreeMap<String, PublicEquipmentStatResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_roll: Option<f64>,
    /// P11 返回含 10 位 key 的真实 URL；历史空值只作兼容，不证明链接可用。
    pub game_equipment_url: String,
}

impl std::fmt::Debug for PublicPreviewEquipmentResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicPreviewEquipmentResponse")
            .field("name", &self.name)
            .field("equipment_group", &self.equipment_group)
            .field("category", &self.category)
            .field("level", &self.level)
            .field("primary_attribute_initials", &self.primary_attribute_initials)
            .field("info_summary", &self.info_summary)
            .field("stats", &self.stats)
            .field("avg_roll", &self.avg_roll)
            .field("game_equipment_url", &"[REDACTED]")
            .finish()
    }
}

fn deserialize_optional_nonempty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(serde::de::Error::custom("可选文本字段不能是空字符串"));
    }
    Ok(Some(value))
}

/// 预览 lot 详情沿用摘要字段，并附加完整公开装备投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPreviewLotDetailResponse {
    #[serde(flatten)]
    pub summary: PublicPreviewLotResponse,
    pub equipment: PublicPreviewEquipmentResponse,
}

/// lot 列表的无标签数组，保持旧 JSON 契约的 preview/live 形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PublicLotListItemsResponse {
    Preview(Vec<PublicPreviewLotResponse>),
    Live(Vec<PublicLiveLotResponse>),
}

/// lot 详情的无标签对象，保持旧 JSON 契约的 preview/live 形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PublicLotDetailItemResponse {
    Preview(PublicPreviewLotDetailResponse),
    Live(PublicLiveLotResponse),
}

/// Public lot 列表与详情共用的响应外壳。
/// 旧 v1 客户端对顶层字段使用严格闭集校验；世界、场次号和计划结束时间只属于
/// `/api/v1/auctions/current`，不能作为扩展字段混入 lot 响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLotEnvelopeResponse {
    pub auction_id: String,
    pub auction_status: String,
    pub projection_kind: String,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub opens_at: String,
    pub preview_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_restriction: Option<PublicFinalRestrictionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lots: Option<PublicLotListItemsResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lot: Option<PublicLotDetailItemResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_counts: Option<BTreeMap<String, i64>>,
}
