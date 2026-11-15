//! Admin 列表共用的数字分页信封。
//!
//! 各列表的分页字段形状完全相同，只在浏览器解码严格度上分为两种：带
//! `deny_unknown_fields` 的严格信封与不带该约束的宽松信封。各列表沿用原有名字作为类型别名，
//! 保持既有解码严格度与 Rust 路径不变。

use serde::{Deserialize, Serialize};

/// 宽松解码的数字分页：浏览器忽略未知字段。只供原本宽松的列表沿用；新列表默认用
/// [`AdminStrictPaginationResponse`]。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// 严格解码的数字分页：浏览器拒绝未知字段。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminStrictPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}
