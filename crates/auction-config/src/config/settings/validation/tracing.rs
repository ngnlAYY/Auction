//! OTLP tracing exporter 和 header 安全校验。

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
    validate_tracing_headers(raw_headers)
}

pub(crate) fn validate_tracing_headers(value: &str) -> Result<(), ConfigError> {
    if value.is_empty() {
        return Ok(());
    }
    if value.len() > 8 * 1024 || value.chars().any(char::is_control) {
        return Err(ConfigError::InvalidTracingConfig(
            "OTLP headers 必须是不超过 8192 字节的安全列表",
        ));
    }
    let parts = value.split(',').collect::<Vec<_>>();
    if parts.len() > 32 {
        return Err(ConfigError::InvalidTracingConfig("OTLP headers 最多包含 32 项"));
    }
    let mut seen = BTreeSet::new();
    for part in parts {
        let Some((key, header_value)) = part.split_once('=') else {
            return Err(ConfigError::InvalidTracingConfig("OTLP headers 必须使用 key=value 格式"));
        };
        let key = key.trim();
        let header_value = header_value.trim();
        if !valid_http_header_token(key)
            || header_value.is_empty()
            || header_value.len() > 1024
            || header_value.chars().any(char::is_control)
        {
            return Err(ConfigError::InvalidTracingConfig("OTLP headers 包含非法或过长的键值"));
        }
        let canonical = key.to_ascii_lowercase();
        if matches!(
            canonical.as_str(),
            "baggage"
                | "connection"
                | "content-encoding"
                | "content-length"
                | "content-type"
                | "host"
                | "traceparent"
                | "tracestate"
                | "transfer-encoding"
        ) || !seen.insert(canonical)
        {
            return Err(ConfigError::InvalidTracingConfig("OTLP headers 含有禁止覆盖或重复的键"));
        }
    }
    Ok(())
}

pub(crate) fn valid_http_header_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}
