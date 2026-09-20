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
    /// 合并入口明确确认在配置保存后初始化专用空库并启动；旧分体请求可省略。
    #[serde(default)]
    pub confirm_initialize: bool,
}

/// 初始化接口的成功阶段：`verified` 仅验证部署码，`validated` 检查配置输入和数据库连接，
/// `completed` 表示写入完成且初始化进程退出；验证响应不授予后续请求的免鉴权资格。
#[derive(Deserialize, Serialize)]
pub struct SetupResponse {
    pub status: String,
    /// 当前初始化表面是否在保存后由 supervisor 初始化并启动；不表示服务已经就绪。
    #[serde(default)]
    pub initialize_and_start: bool,
}

/// 不包含底层数据库、TOML 或文件内容的失败响应。
#[derive(Deserialize, Serialize)]
pub struct SetupErrorResponse {
    pub code: String,
}
