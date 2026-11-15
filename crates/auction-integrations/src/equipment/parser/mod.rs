//! 装备来源解析出口；HTML、header 协议与领域名称识别保持单独职责。

mod header;
mod html;
mod identity;
mod page;
mod stats;
mod text;
mod url;

// 门面：装备解析失败沿用应用层的来源明细码闭集，equipment 各子模块统一经此别名引用。
pub(super) use auction_application::mail::EquipmentSourceDetailCode as EquipmentParseError;
pub(super) use header::EquipmentLevel;
pub(super) use identity::parse_identity;
pub(super) use page::parse_equipment_page;
pub(super) use stats::normalize_stat_value;
pub(super) use url::parse_equipment_url;
