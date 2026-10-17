//! Worker control 的应用端口与安全状态投影。
//! Unix socket、HTTP/JSON 和进程凭据校验属于外层适配器；应用层只接收有界的运行状态，避免
//! Admin dashboard 直接依赖 Worker 的网络实现。手动轮询是唤醒提示，不替代邮件命令账本、
//! 游标或入站事务。

use super::ApplicationFuture;
use uuid::Uuid;

/// Worker control client 读取失败时向 Admin 暴露的有限分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerControlCondition {
    /// Worker control 返回了符合当前协议的状态。
    Available,
    /// socket 不存在、连接失败或对端主动不可用。
    Unavailable,
    /// Worker control 请求超过了有界超时。
    Timeout,
    /// 对端协议版本或响应形状不受支持。
    Incompatible,
    /// 对端明确拒绝了请求。
    Rejected,
}

impl WorkerControlCondition {
    /// 返回 Admin/JSON 契约中的稳定条件名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Incompatible => "incompatible",
            Self::Rejected => "rejected",
        }
    }
}

/// Worker control 的不含敏感值状态摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerControlStatus {
    pub condition: WorkerControlCondition,
    pub state: String,
    pub ready: bool,
    pub version: String,
    pub mail_pollers: i64,
}

impl WorkerControlStatus {
    /// 构造失败状态；失败时清空对端字段，避免展示过期或未验证的 ready 信息。
    pub fn unavailable(condition: WorkerControlCondition) -> Self {
        Self {
            condition,
            state: String::new(),
            ready: false,
            version: String::new(),
            mail_pollers: 0,
        }
    }
}

/// Worker 手动唤醒的安全结果；唤醒失败不否定已经写入数据库的 due 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerControlTrigger {
    pub condition: WorkerControlCondition,
    pub accepted: bool,
    pub pollers: i64,
}

/// Admin dashboard 读取 Worker control 的窄端口。
pub trait WorkerControlPort: Send + Sync {
    fn status(&self) -> ApplicationFuture<'_, WorkerControlStatus>;

    fn trigger_poll(&self, command_id: Uuid) -> ApplicationFuture<'_, WorkerControlTrigger>;
}
