//! 仅用于本机首次部署的初始化请求与稳定结果码。
//!
//! 初始化接口不属于常规 Admin API；正式进程不挂载这些路由。

use serde::{Deserialize, Serialize};

/// 一次性初始化的数据库输入；与配置模型分离，不返回到响应或常规管理 DTO。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupDatabaseRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub name: String,
}

/// 一次性初始化的表单内容；数据库凭据和管理员密码只能出现在请求及目标敏感 TOML 中。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupRequest {
    pub database: SetupDatabaseRequest,
    pub admin_actor: String,
    pub admin_secret: String,
    pub public_origin: String,
    pub admin_origin: String,
    pub metrics_enabled: bool,
    pub schedule_enabled: bool,
    pub start_price: i64,
    pub log_filter: String,
    pub worker_concurrency: u32,
    pub worker_poll_interval_seconds: u64,
    pub realtime_poll_interval_millis: u64,
}

/// 初始化接口的成功阶段；完成后初始化进程退出。
#[derive(Deserialize, Serialize)]
pub struct SetupResponse {
    pub status: String,
}

/// 不包含底层数据库、TOML 或文件内容的失败响应。
#[derive(Deserialize, Serialize)]
pub struct SetupErrorResponse {
    pub code: String,
}
