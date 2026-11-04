//! 闭集行为开关及持久事实端口；只选择正确路径，不承载业务授权。

mod commands;
mod snapshot;

pub use commands::*;
pub use snapshot::*;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

/// 使用开关的进程角色；保持指标标签闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSwitchRole {
    /// 公开站点。
    Public,
    /// 管理站点。
    Admin,
    /// 后台工作进程。
    Worker,
}

impl RuntimeSwitchRole {
    /// 固定角色名称。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Admin => "admin",
            Self::Worker => "worker",
        }
    }
}

/// 本版认识的名称；数据库只检查格式，旧程序显式忽略未来名称。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSwitchName {
    /// 审计失败对就绪的投影。
    ReadinessAuditFailureMode,
    /// HTTP 最终就绪结果的短缓存。
    ReadinessCacheEnabled,
    /// Worker 的附加只读 SLI 汇总。
    MetricsSliSnapshotEnabled,
    /// Worker 的 MoogleMail 收件与维护隔离。
    MooglemailPollIsolationEnabled,
    /// 指标路由拒绝携带转发痕迹的请求。
    MetricsRejectForwardedRequests,
}

impl RuntimeSwitchName {
    /// 本版完整名称集。
    pub const ALL: [Self; 5] = [
        Self::ReadinessAuditFailureMode,
        Self::ReadinessCacheEnabled,
        Self::MetricsSliSnapshotEnabled,
        Self::MooglemailPollIsolationEnabled,
        Self::MetricsRejectForwardedRequests,
    ];
    /// 稳定持久名称与指标标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::ReadinessAuditFailureMode => "readiness_audit_failure_mode",
            Self::ReadinessCacheEnabled => "readiness_cache_enabled",
            Self::MetricsSliSnapshotEnabled => "metrics_sli_snapshot_enabled",
            Self::MooglemailPollIsolationEnabled => "mooglemail_poll_isolation_enabled",
            Self::MetricsRejectForwardedRequests => "metrics_reject_forwarded_requests",
        }
    }
    /// 在解码值之前识别名称；未知名称不是损坏数据。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|name| name.label() == value)
    }
    /// 名称与消费角色绑定；不适用的进程只展示数据库意图。
    pub fn applies_to(self, role: RuntimeSwitchRole) -> bool {
        match self {
            Self::ReadinessAuditFailureMode => true,
            Self::ReadinessCacheEnabled => role != RuntimeSwitchRole::Worker,
            Self::MetricsSliSnapshotEnabled => role == RuntimeSwitchRole::Worker,
            Self::MooglemailPollIsolationEnabled => role == RuntimeSwitchRole::Worker,
            Self::MetricsRejectForwardedRequests => true,
        }
    }
    /// 闭集名称绑定值类型，拒绝已知名称下的未知枚举或类型。
    pub fn valid_value(self, value: &serde_json::Value) -> bool {
        match self {
            Self::ReadinessAuditFailureMode => value
                .as_str()
                .is_some_and(|value| matches!(value, "legacy" | "shadow" | "classified")),
            Self::ReadinessCacheEnabled
            | Self::MetricsSliSnapshotEnabled
            | Self::MooglemailPollIsolationEnabled
            | Self::MetricsRejectForwardedRequests => value.is_boolean(),
        }
    }
}

/// 一次数据库读取的同源时间与最新事实。
#[derive(Debug, Clone)]
pub struct RuntimeSwitchRead {
    /// 数据库读取时刻，用于推算期限，不依赖进程墙钟跳变。
    pub database_time: DateTime<Utc>,
    /// 每个已知名称最多一个最新事实；不能先过滤过期行。
    pub events: Vec<RuntimeSwitchEvent>,
}

/// 开关 owner 的稳定错误，不携带驱动原文或秘密值。
#[derive(Debug, thiserror::Error)]
pub enum RuntimeSwitchError {
    /// 输入无效。
    #[error("运行时开关命令无效")]
    Invalid,
    /// 同 ID 异参或过期版本。
    #[error("运行时开关命令或版本冲突；重新读取后使用新命令 ID")]
    Conflict,
    /// 已知名称的事实损坏。
    #[error("运行时开关事实无效；保留最后成功快照")]
    Corrupt,
    /// 数据库不可用。
    #[error("运行时开关存储不可用")]
    Unavailable,
    /// 提交回执无法确认，不授权新命令替代重试。
    #[error("运行时开关提交结果未知；必须使用原命令 ID 和全部原参数原样重试")]
    OutcomeUnknown,
    /// 安全策略只能在线收紧，放宽必须回到部署配置。
    #[error("指标转发拒绝只能在线收紧；放宽须修改部署配置并重启")]
    UnsafeRelaxation,
}

/// 窄端口的异步结果。
pub type RuntimeSwitchFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RuntimeSwitchError>> + Send + 'a>>;

/// 存储只拥有 SQL、锁和事务；解析、摘要与状态选择由应用类型定义。
pub trait RuntimeSwitchStore: Send + Sync {
    /// 读取全部已知名称的最新事实。
    fn read(&self) -> RuntimeSwitchFuture<'_, RuntimeSwitchRead>;
    /// 同事务接收事实、命令结果和审计。
    fn write(&self, command: RuntimeSwitchCommand) -> RuntimeSwitchFuture<'_, RuntimeSwitchEvent>;
}
