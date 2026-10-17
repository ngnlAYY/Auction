//! 仅用于本机首次部署的初始化请求与稳定结果码。
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

/// 首次部署的本地周排期；省略对象或其中字段时沿用既有模板，不接受 world 或 runtime 接管参数。
/// 容器级 `serde(default)` 用本结构的 `Default` 补齐缺项，而非使用空字符串。
#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetupScheduleRequest {
    /// IANA 时区名称；不按浏览器时区解释下方本地时间。
    pub timezone: String,
    pub preview_day: String,
    /// 预览本地时间，必须为精确 HH:MM。
    pub preview_time: String,
    /// 同一周内的开拍星期，使用英文完整名称。
    pub open_day: String,
    /// 开拍本地时间；必须至少比预览晚 120 分钟。
    pub open_time: String,
    /// 同一周内的名义结束星期，使用英文完整名称。
    pub nominal_end_day: String,
    /// 名义结束本地时间，必须晚于开拍；不改变软截止规则。
    pub nominal_end_time: String,
}

impl Default for SetupScheduleRequest {
    fn default() -> Self {
        Self {
            timezone: "UTC".to_owned(),
            preview_day: "Thursday".to_owned(),
            preview_time: "22:00".to_owned(),
            open_day: "Friday".to_owned(),
            open_time: "00:00".to_owned(),
            nominal_end_day: "Saturday".to_owned(),
            nominal_end_time: "12:00".to_owned(),
        }
    }
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
    /// 正式 Public 服务的回环 TCP 端口；省略时沿用 8090，不改变设置入口。
    #[serde(default = "default_public_listener_port")]
    pub public_listener_port: u16,
    /// 正式 Admin 服务的回环 TCP 端口；省略时沿用 8091，不能与 Public 相同。
    #[serde(default = "default_admin_listener_port")]
    pub admin_listener_port: u16,
    pub metrics_enabled: bool,
    pub schedule_enabled: bool,
    /// 排期关闭时仍保存并校验候选；启用开关继续使用既有顶层字段。
    #[serde(default)]
    pub schedule: SetupScheduleRequest,
    pub start_price: i64,
    pub log_filter: String,
    pub worker_concurrency: u32,
    pub worker_poll_interval_seconds: u64,
    pub realtime_poll_interval_millis: u64,
    /// 合并入口明确确认在配置保存后初始化专用空库并启动；旧分体请求可省略。
    #[serde(default)]
    pub confirm_initialize: bool,
}

fn default_public_listener_port() -> u16 {
    8090
}

fn default_admin_listener_port() -> u16 {
    8091
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
