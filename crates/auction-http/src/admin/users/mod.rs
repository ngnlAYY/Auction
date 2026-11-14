//! Admin 用户 HTTP 适配器的模块装配。
//! 用户列表、用户命令（含改名回退）、黑名单动作、DTO 映射和共享错误边界分别由子模块拥有；
//! 本模块只转出路由可见的稳定处理器与子模块之间共用的 DTO 映射。会话守卫与状态由子模块
//! 从定义处导入。

mod boundary;
mod commands;
mod list;
mod mapping;
mod status;

pub(crate) use commands::{create, mutate, revert_rename};
pub(crate) use list::list;
pub(crate) use mapping::{map_list_user, map_user};
pub(crate) use status::{blacklist, unblacklist};
