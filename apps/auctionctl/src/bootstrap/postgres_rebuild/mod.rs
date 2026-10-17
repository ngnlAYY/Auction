//! PostgreSQL 数据库破坏性重建模块装配；安全边界由参数授权、执行、
//! 后置验证和不可覆盖回执 owner 持有。

mod execute;
mod model;
mod postconditions;
mod receipt;
mod validation;

pub(super) use execute::execute;
pub(super) use model::{Mode, Options};
pub(crate) use validation::{application_database_username, resolve_maintenance_database_url};
