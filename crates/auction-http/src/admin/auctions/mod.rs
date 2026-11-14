//! Admin 拍卖 HTTP 适配器的模块装配。
//! 拍卖列表、创建命令、DTO 映射和共享错误边界分别由子模块拥有；本模块只保留路由可见的
//! 稳定处理器出口和跨子模块使用的 HTTP 辅助导入。

mod commands;
mod errors;
mod list;
mod mapping;
mod schedule;

pub(crate) use super::{
    AdminApiState, admin_session_guard, authenticated_admin_mutation, authenticated_admin_session,
    internal_error, no_store_headers,
};
pub(crate) use commands::create;
pub(crate) use errors::{feature_unavailable, invalid_create_command, map_create_error, map_error};
pub(crate) use list::list;
pub(crate) use mapping::{map_auction, map_auction_command};
pub(crate) use schedule::update as update_schedule;
