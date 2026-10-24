//! OTLP tracing exporter 和 header 安全校验。

use std::borrow::Cow;

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
    if value.split(',').count() > 32 {
        return Err(ConfigError::InvalidTracingConfig("OTLP headers 最多包含 32 项"));
    }
    let mut seen = BTreeSet::new();
    for part in value.split(',') {
        let Some((key, header_value)) = part.split_once('=') else {
            return Err(ConfigError::InvalidTracingConfig("OTLP headers 必须使用 key=value 格式"));
        };
        // exporter 按百分号解码后校验；配置预检必须使用同一名称、长度和控制字符边界。
        let key = decode_tracing_header(key.trim())?;
        let header_value = decode_tracing_header(header_value.trim())?;
        if !valid_http_header_token(&key)
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

/// 严格解码 OTLP 的 URL 百分号形式；普通值只借用原文，不把 `+` 当作表单空格。
fn decode_tracing_header(value: &str) -> Result<Cow<'_, str>, ConfigError> {
    if !value.as_bytes().contains(&b'%') {
        return Ok(Cow::Borrowed(value));
    }
    let invalid = || ConfigError::InvalidTracingConfig("OTLP headers 百分号编码无效");
    let mut bytes = value.bytes();
    let mut decoded = Vec::with_capacity(value.len());
    while let Some(byte) = bytes.next() {
        let byte = if byte == b'%' {
            let high =
                bytes.next().and_then(|byte| char::from(byte).to_digit(16)).ok_or_else(invalid)?;
            let low =
                bytes.next().and_then(|byte| char::from(byte).to_digit(16)).ok_or_else(invalid)?;
            (high * 16 + low) as u8
        } else {
            byte
        };
        decoded.push(byte);
    }
    String::from_utf8(decoded).map(Cow::Owned).map_err(|_| invalid())
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
