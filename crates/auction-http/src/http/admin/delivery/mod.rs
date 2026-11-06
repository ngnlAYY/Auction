//! Admin delivery HTTP 适配器的模块装配。
//! delivery 列表、人工命令、快照映射和共享能力/错误边界分别由子模块拥有；本模块只保留
//! 路由可见的稳定处理器出口和跨子模块使用的 HTTP 辅助导入。

mod commands;
mod common;
mod detail;
mod list;
mod mapping;
mod sent_proof;
mod support;

pub(crate) use super::{
    AdminApiState, admin_session_guard, api_error, authenticated_admin_mutation,
    authenticated_admin_session, internal_error, no_store_headers, parse_positive_i64, parse_query,
    request_id, timestamp,
};
pub(crate) use commands::{reconcile, release, retry};
pub(crate) use common::{
    feature_unavailable, map_delivery_error, parse_optional_filter, parse_optional_reference,
    parse_optional_timestamp,
};
pub(crate) use detail::detail;
pub(crate) use list::list;
pub(crate) use mapping::{
    map_detail, map_player_support, map_sent_proof_preview, map_sent_proof_result, map_snapshot,
};
pub(crate) use sent_proof::{confirm as sent_proof_confirm, preview as sent_proof_preview};
pub(crate) use support::player_support;
