//! 敏感配置子模块的稳定出口；模型、运维维护文件、`.keyenv` 解析和 Admin runtime
//! 投影由相邻 owner 持有。

mod database;
mod keyenv;
mod maintenance;
mod model;
mod runtime;

pub use database::PostgresConnectionConfig;
pub use keyenv::load_mooglemail_keyenv;
pub use maintenance::load_postgres_maintenance_secrets;
pub(super) use model::{AppSecrets, SecretString};
pub use model::{MoogleMailKeyEnv, PostgresMaintenanceSecrets};
pub(super) use runtime::{
    load_admin_runtime_file, load_secret_file, runtime_http_security, runtime_session,
};
