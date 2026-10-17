//! tracing exporter 的运行配置与初始化/关闭错误。

use std::time::Duration;

use thiserror::Error;

/// tracing 导出所需的非敏感运行参数。
/// 端点只允许由上层配置模型校验后的 HTTP(S) 地址提供；本结构不承载请求 headers、Cookie
/// 或其他凭据，避免把秘密材料带入可观测性边界。headers 只在初始化时短暂传入 exporter。
#[derive(Debug, Clone)]
pub struct TracingConfig {
    pub enabled: bool,
    /// OTLP/HTTP collector 地址；导出器会按 OTLP 约定补充 `/v1/traces`。
    pub endpoint: String,
    pub batch_timeout: Duration,
    pub export_timeout: Duration,
    pub queue_capacity: usize,
    pub max_export_batch_size: usize,
    /// 父级采样比例，范围为 `0.0..=1.0`。
    pub sample_ratio: f64,
    pub shutdown_timeout: Duration,
}

#[derive(Debug, Error)]
#[error("failed to initialize tracing subscriber: {message}")]
pub struct InitError {
    pub(crate) message: String,
}

/// tracing exporter 关闭错误。
#[derive(Debug, Error)]
#[error("failed to shut down tracing exporter: {message}")]
pub struct ShutdownError {
    pub(crate) message: String,
}
