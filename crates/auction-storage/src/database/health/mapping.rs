//! 服务心跳的闭集字段与安全错误映射。

use auction_observability::{
    HealthError, HeartbeatStatus, ProcessRole, ServiceName, TracingStatus,
};

pub(super) fn parse_service(value: String) -> Result<ServiceName, HealthError> {
    ServiceName::parse(&value).ok_or_else(|| storage_error("服务心跳名称无效"))
}

pub(super) fn parse_role(value: String) -> Result<ProcessRole, HealthError> {
    ProcessRole::parse(&value).ok_or_else(|| storage_error("服务心跳职责无效"))
}

pub(super) fn parse_status(value: String) -> Result<HeartbeatStatus, HealthError> {
    HeartbeatStatus::parse(&value).ok_or_else(|| storage_error("服务心跳状态无效"))
}

pub(super) fn parse_tracing(value: String) -> Result<TracingStatus, HealthError> {
    TracingStatus::parse(&value).ok_or_else(|| storage_error("tracing 状态无效"))
}

pub(super) fn storage_error(message: &str) -> HealthError {
    HealthError::Storage { message: message.to_owned() }
}

/// SQLx 错误仅附加允许的诊断字段，健康错误变体与失败关闭语义保持不变。
#[cfg(feature = "postgres")]
pub(super) fn storage_error_from(error: sqlx::Error, context: &'static str) -> HealthError {
    HealthError::Storage { message: crate::database::sql_error_context(&error, context) }
}
