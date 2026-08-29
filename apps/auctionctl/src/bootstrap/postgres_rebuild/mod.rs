//! PostgreSQL 数据库破坏性重建模块装配。
//!
//! 破坏性命令的安全边界分别由参数/授权、执行、后置验证和不可覆盖回执 owner 持有；
//! 本文件只保留稳定调用路径，避免把安全决策隐藏在巨型装配模块中。

mod execute;
mod model;
mod postconditions;
mod receipt;
mod validation;

pub(super) use execute::execute;
pub(super) use model::{Mode, Options};
pub(crate) use validation::{application_database_username, resolve_maintenance_database_url};
