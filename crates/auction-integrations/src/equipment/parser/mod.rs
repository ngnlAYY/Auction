//! 旧版装备页解析的稳定出口。
//!
//! URL/来源约束、HTML 页面结构、名称身份映射和统计规范化分别归属相邻模块；本文件只
//! 负责装配，避免 `reader`、`model` 和 `config` 依赖具体解析实现。

mod common;
mod identity;
mod page;
mod stats;
mod url;

pub(super) use identity::{category_for_type, parse_identity};
pub(super) use page::parse_equipment_page;
pub(super) use stats::normalize_stat_value;
pub(super) use url::{parse_equipment_url, valid_dns_name, valid_path_prefix};
