//! Admin 用户 HTTP 适配器的模块装配。
//!
//! 用户列表、用户命令、黑名单/改名状态动作、DTO 映射和共享错误边界分别由子模块拥有；
//! 本模块只保留稳定处理器出口和跨子模块使用的 HTTP 辅助导入。

#[path = "commands.rs"]
mod commands;
#[path = "common.rs"]
mod common;
#[path = "list.rs"]
mod list;
#[path = "mapping.rs"]
mod mapping;
#[path = "status.rs"]
mod status;

pub(super) use super::{
    ADMIN_CSRF_COOKIE, AdminApiState, admin_session_guard, api_error, authenticated_admin_session,
    authentication_error, internal_error, not_found_error, parse_positive_i64, parse_query,
    read_admin_cookie, request_id, timestamp, valid_admin_csrf_header,
};
pub(super) use commands::{create, mutate, revert_rename};
pub(super) use list::list;
pub(super) use mapping::{map_list_user, map_user};
pub(super) use status::{blacklist, unblacklist};
