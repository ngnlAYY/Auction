//! Admin delivery HTTP 适配器的模块装配。
//!
//! delivery 列表、人工命令、快照映射和共享能力/错误边界分别由子模块拥有；本模块只保留
//! 路由可见的稳定处理器出口和跨子模块使用的 HTTP 辅助导入。

#[path = "commands.rs"]
mod commands;
#[path = "common.rs"]
mod common;
#[path = "list.rs"]
mod list;
#[path = "mapping.rs"]
mod mapping;

pub(super) use super::{
    AdminApiState, admin_session_guard, api_error, authenticated_admin_mutation,
    authenticated_admin_session, internal_error, no_store_headers, parse_positive_i64, parse_query,
    request_id, timestamp,
};
pub(super) use commands::{reconcile, release, retry};
pub(super) use common::{feature_unavailable, map_delivery_error};
pub(super) use list::list;
pub(super) use mapping::map_snapshot;
