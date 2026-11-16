//! Admin delivery HTTP 适配器的模块装配。
//! delivery 列表、人工命令、快照映射、列表与详情的响应投影和共享能力/错误边界分别由子模块
//! 拥有；本模块只转出路由可见的稳定处理器，以及子模块之间共用的能力/错误边界、过滤参数解析与
//! 快照映射。会话守卫与状态由子模块从定义处导入。

mod boundary;
mod commands;
mod detail;
mod list;
mod mapping;
mod projection;
mod sent_proof;
mod support;

pub(crate) use boundary::{
    feature_unavailable, map_delivery_error, parse_optional_filter, parse_optional_reference,
    parse_optional_timestamp,
};
pub(crate) use commands::{acknowledge, reconcile, release, retry};
pub(crate) use detail::detail;
pub(crate) use list::list;
pub(crate) use mapping::{
    map_detail, map_player_support, map_sent_proof_preview, map_sent_proof_result, map_snapshot,
};
pub(crate) use sent_proof::{confirm as sent_proof_confirm, preview as sent_proof_preview};
pub(crate) use support::player_support;
