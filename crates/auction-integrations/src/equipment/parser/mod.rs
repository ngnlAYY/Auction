//! 旧版装备页解析的稳定出口。

mod common;
mod identity;
mod page;
mod stats;
mod url;

pub(super) use identity::{category_for_type, parse_identity};
pub(super) use page::parse_equipment_page;
pub(super) use stats::normalize_stat_value;
pub(super) use url::{parse_equipment_url, valid_dns_name, valid_path_prefix};
