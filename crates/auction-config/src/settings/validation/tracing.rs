//! OTLP tracing exporter 和 header 安全校验。

use auction_application::otlp_headers::parse_otlp_headers;

use super::super::*;

pub(crate) fn validate_tracing(
    config: &TracingSettings,
    headers: Option<&str>,
) -> Result<(), ConfigError> {
    if !(1..=30).contains(&config.batch_timeout_seconds)
        || !(1..=30).contains(&config.export_timeout_seconds)
        || !(1..=30).contains(&config.shutdown_timeout_seconds)
        || !(1..=512).contains(&config.max_export_batch_size)
        || !(1..=4096).contains(&config.queue_capacity)
        || config.max_export_batch_size > config.queue_capacity
        || config.enabled && (config.sample_ratio_ppm == 0 || config.sample_ratio_ppm > 1_000_000)
        || !config.enabled && config.sample_ratio_ppm > 1_000_000
    {
        return Err(ConfigError::InvalidTracingConfig("tracing 数值边界无效"));
    }

    // SDK 的环境头会覆盖显式配置；在配置边界拒绝，仅检查是否设置，不修改全局环境或回显值。
    if config.enabled
        && ["OTEL_EXPORTER_OTLP_HEADERS", "OTEL_EXPORTER_OTLP_TRACES_HEADERS"]
            .iter()
            .any(|key| std::env::var_os(key).is_some())
    {
        return Err(ConfigError::InvalidTracingConfig(
            "启用 tracing 前须移除 OTEL_EXPORTER_OTLP_HEADERS 和 OTEL_EXPORTER_OTLP_TRACES_HEADERS；导出头仅允许敏感 TOML 配置",
        ));
    }

    let raw_headers = headers.unwrap_or_default();
    if !config.enabled && !raw_headers.is_empty() {
        return Err(ConfigError::InvalidTracingConfig("关闭 tracing 时不能提供 OTLP headers"));
    }
    if config.enabled && config.endpoint.is_empty() {
        return Err(ConfigError::InvalidTracingConfig("启用 tracing 时 endpoint 不能为空"));
    }
    if !config.endpoint.is_empty() {
        let parsed = Url::parse(&config.endpoint)
            .map_err(|_| ConfigError::InvalidTracingConfig("endpoint 必须是绝对 HTTP(S) 地址"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.as_str() != config.endpoint
            || config.endpoint.chars().any(char::is_control)
        {
            return Err(ConfigError::InvalidTracingConfig(
                "endpoint 不能包含 userinfo、query、fragment 或控制字符",
            ));
        }
    }
    // exporter 初始化用同一个解析函数，预检通过即保证运行期接受。
    parse_otlp_headers(raw_headers)
        .map(drop)
        .map_err(|error| ConfigError::InvalidTracingConfig(error.reason()))
}
