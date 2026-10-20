//! 装备来源解析出口；HTML、header 协议与领域名称识别保持单独职责。

mod common;
mod header;
mod html;
mod identity;
mod page;
mod stats;
mod url;

pub(super) use auction_application::EquipmentSourceDetailCode as EquipmentParseError;
pub(super) use header::EquipmentLevel;
pub(super) use identity::parse_identity;
pub(super) use page::parse_equipment_page;
pub(super) use stats::normalize_stat_value;
pub(super) use url::{parse_equipment_url, valid_dns_name, valid_path_prefix};
