//! Admin 凭据管理 HTTP 适配器的模块装配。
//!
//! 凭据列表、写命令、DTO 映射和共享安全/错误边界分别由子模块拥有；本模块只保留路由
//! 可见的稳定处理器出口和跨子模块使用的 HTTP 辅助导入。

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
    authenticated_admin_session, internal_error, no_store_headers, not_found_error,
    parse_positive_i64, parse_query, request_id, timestamp,
};
pub(super) use commands::{create, delete, mutate};
pub(super) use common::{
    feature_unavailable, invalid_json, invalid_query, invalid_request, map_error, mutation_boundary,
};
pub(super) use list::list;
pub(super) use mapping::map_credential;
