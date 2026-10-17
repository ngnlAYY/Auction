use std::{future::Future, pin::Pin};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use thiserror::Error;

/// 心跳 details 的最大序列化大小，避免健康记录成为任意 JSON 存储通道。
pub const MAX_HEARTBEAT_DETAILS_BYTES: usize = 64 << 10;

pub const MAX_HEARTBEAT_TTL: Duration = Duration::hours(24);

pub type HealthFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, HealthError>> + Send + 'a>>;

/// 心跳运行状态读取器的异步 readiness 返回值；失败依赖直接投影为 false，不泄漏错误细节。
pub type HeartbeatReadinessFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// 健康存储或安全投影失败。
#[derive(Debug, Error)]
pub enum HealthError {
    #[error("健康命令字段无效")]
    InvalidCommand,

    #[error("健康数据投影无效")]
    InvalidData,

    #[error("心跳 observation sequence 已过期")]
    StaleObservation,

    #[error("健康存储不可用: {message}")]
    Storage { message: String },
}

/// 进程服务名称，与 service_heartbeats 的数据库闭集一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServiceName {
    Public,
    Admin,
    Worker,
}

impl ServiceName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "auction-public",
            Self::Admin => "auction-admin",
            Self::Worker => "auction-worker",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auction-public" => Some(Self::Public),
            "auction-admin" => Some(Self::Admin),
            "auction-worker" => Some(Self::Worker),
            _ => None,
        }
    }

    pub(super) const fn role(self) -> ProcessRole {
        match self {
            Self::Public => ProcessRole::Public,
            Self::Admin => ProcessRole::Admin,
            Self::Worker => ProcessRole::Worker,
        }
    }

    pub(super) const ALL: [Self; 3] = [Self::Public, Self::Admin, Self::Worker];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcessRole {
    Public,
    Admin,
    Worker,
}

impl ProcessRole {
    /// 返回稳定的外部名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Admin => "admin",
            Self::Worker => "worker",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "public" => Some(Self::Public),
            "admin" => Some(Self::Admin),
            "worker" => Some(Self::Worker),
            _ => None,
        }
    }
}

/// 心跳允许跨进程公开的进程状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatStatus {
    Starting,
    Ready,
    Degraded,
    Stopping,
}

impl HeartbeatStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Stopping => "stopping",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "starting" => Some(Self::Starting),
            "ready" => Some(Self::Ready),
            "degraded" => Some(Self::Degraded),
            "stopping" => Some(Self::Stopping),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TracingStatus {
    Disabled,
    Ok,
    Degraded,
}

impl TracingStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Ok => "ok",
            Self::Degraded => "degraded",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "ok" => Some(Self::Ok),
            "degraded" => Some(Self::Degraded),
            _ => None,
        }
    }
}

/// 不包含 exporter 配置、原始错误或 span 内容的 tracing 安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracingHealth {
    pub status: TracingStatus,
    pub dropped_span_count: i64,
    pub export_failure_count: i64,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_error_code: String,
}

impl TracingHealth {
    /// 校验 tracing 计数、时间和错误码的组合，避免把损坏状态展示为健康。
    pub fn validate(&self) -> bool {
        if self.dropped_span_count < 0
            || self.export_failure_count < 0
            || self.last_success_at.is_some_and(|value| value.timestamp_micros() == 0)
            || self.last_failure_at.is_some_and(|value| value.timestamp_micros() == 0)
            || !matches!(
                self.last_error_code.as_str(),
                "" | "timeout" | "canceled" | "transport" | "rejected" | "shutdown"
            )
        {
            return false;
        }
        if self.status == TracingStatus::Disabled {
            return self.dropped_span_count == 0
                && self.export_failure_count == 0
                && self.last_success_at.is_none()
                && self.last_failure_at.is_none()
                && self.last_error_code.is_empty();
        }
        if self.export_failure_count == 0 && self.last_failure_at.is_some()
            || self.export_failure_count > 0 && self.last_failure_at.is_none()
            || !self.last_error_code.is_empty()
                && (self.status != TracingStatus::Degraded || self.last_failure_at.is_none())
            || self.status == TracingStatus::Degraded
                && self.dropped_span_count == 0
                && self.last_error_code.is_empty()
        {
            return false;
        }
        self.status != TracingStatus::Ok
            || self.last_failure_at.is_none()
            || self.last_success_at.is_some_and(|success| {
                self.last_failure_at.is_none_or(|failure| success >= failure)
            })
    }
}

pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(super) fn valid_details(value: &Value) -> bool {
    value.is_object() && value.to_string().len() <= MAX_HEARTBEAT_DETAILS_BYTES
}
