//! Admin 拍卖 HTTP 适配器的模块装配。
//! 拍卖列表、创建命令、DTO 映射和共享错误边界分别由子模块拥有；本模块只转出路由可见的
//! 稳定处理器、`lots` 复用的拍卖映射，以及子模块之间共用的错误边界与映射。会话守卫与状态
//! 由子模块从定义处导入。

mod commands;
mod errors;
mod list;
mod mapping;
mod schedule;

pub(crate) use commands::create;
#[cfg(test)]
pub(super) use commands::tests as command_test_support;
pub(crate) use errors::{feature_unavailable, invalid_create_command, map_create_error, map_error};
pub(crate) use list::list;
pub(crate) use mapping::{map_auction, map_auction_command};
pub(crate) use schedule::update as update_schedule;
