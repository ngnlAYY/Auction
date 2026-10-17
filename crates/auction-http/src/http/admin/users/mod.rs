//! Admin 用户 HTTP 适配器的模块装配。
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
    AdminApiState, admin_session_guard, api_error, authenticated_admin_mutation,
    authenticated_admin_session, internal_error, parse_positive_i64, parse_query, request_id,
    timestamp,
};
pub(super) use commands::{create, mutate, revert_rename};
pub(super) use list::list;
pub(super) use mapping::{map_list_user, map_user};
pub(super) use status::{blacklist, unblacklist};
