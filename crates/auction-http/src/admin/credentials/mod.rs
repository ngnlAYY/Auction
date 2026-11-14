//! Admin 凭据管理 HTTP 适配器的模块装配。
//! 凭据列表、写命令、DTO 映射和共享安全/错误边界分别由子模块拥有；本模块只转出路由可见的
//! 稳定处理器，以及子模块之间共用的安全/错误边界与 DTO 映射。会话守卫与状态由子模块从
//! 定义处导入。

mod boundary;
mod check;
mod commands;
mod list;
mod mapping;

pub(crate) use boundary::{
    feature_unavailable, invalid_json, invalid_query, invalid_request, map_error, mutation_boundary,
};
pub(crate) use check::check;
pub(crate) use commands::{create, delete, mutate};
pub(crate) use list::list;
pub(crate) use mapping::map_credential;
