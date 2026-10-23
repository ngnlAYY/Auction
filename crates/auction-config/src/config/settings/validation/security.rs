//! 缓存、HTTP 安全、realtime 和会话配置校验。

use super::super::*;

pub(crate) fn validate_cache(config: &CacheConfig) -> Result<(), ConfigError> {
    if !(1..=3600).contains(&config.public_rules_ttl_seconds) {
        return Err(ConfigError::InvalidCacheConfig("public_rules_ttl_seconds 必须位于 1 至 3600"));
    }
    Ok(())
}

pub(crate) fn validate_http_security(config: &HttpSecurityConfig) -> Result<(), ConfigError> {
    if config.login_rate_limit == 0 || config.login_rate_limit > 10_000 {
        return Err(ConfigError::InvalidHttpSecurity("login_rate_limit 必须位于 1 至 10000"));
    }
    if config.mutation_rate_limit == 0 || config.mutation_rate_limit > 10_000 {
        return Err(ConfigError::InvalidHttpSecurity("mutation_rate_limit 必须位于 1 至 10000"));
    }
    if !(1..=3600).contains(&config.rate_limit_window_seconds) {
        return Err(ConfigError::InvalidHttpSecurity(
            "rate_limit_window_seconds 必须位于 1 至 3600",
        ));
    }
    if !(1..=120).contains(&config.request_timeout_seconds) {
        return Err(ConfigError::InvalidHttpSecurity("request_timeout_seconds 必须位于 1 至 120"));
    }
    if config.trusted_proxy_cidrs.len() > 64 {
        return Err(ConfigError::InvalidHttpSecurity("trusted_proxy_cidrs 最多包含 64 个网段"));
    }
    for (index, value) in config.trusted_proxy_cidrs.iter().enumerate() {
        let network = value
            .parse::<IpNet>()
            .map_err(|_| ConfigError::InvalidHttpSecurity("trusted_proxy_cidrs 必须是规范 CIDR"))?;
        if network.trunc().to_string() != *value
            || config.trusted_proxy_cidrs[..index].iter().any(|existing| existing == value)
        {
            return Err(ConfigError::InvalidHttpSecurity("trusted_proxy_cidrs 必须规范且不能重复"));
        }
    }
    if config.allowed_origins_enabled {
        if config.allowed_origins.is_empty() || config.allowed_origins.len() > 16 {
            return Err(ConfigError::InvalidHttpSecurity(
                "allowed_origins 启用时必须包含 1 至 16 个 origin",
            ));
        }
        for (index, origin) in config.allowed_origins.iter().enumerate() {
            let parsed = Url::parse(origin).map_err(|_| {
                ConfigError::InvalidHttpSecurity("allowed_origins 必须是有效的 HTTP(S) origin")
            })?;
            let normalized_origin = origin.strip_suffix('/').unwrap_or(origin);
            let serialized_origin = parsed.as_str();
            let serialized_origin =
                serialized_origin.strip_suffix('/').unwrap_or(serialized_origin);
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || parsed.username() != ""
                || parsed.password().is_some()
                || !(parsed.path().is_empty() || parsed.path() == "/")
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || serialized_origin != normalized_origin
                || config.allowed_origins[..index].iter().any(|existing| {
                    existing.strip_suffix('/').unwrap_or(existing) == normalized_origin
                })
            {
                return Err(ConfigError::InvalidHttpSecurity(
                    "allowed_origins 只能包含唯一的精确 scheme+host",
                ));
            }
        }
    }
    Ok(())
}

/// 当启用 origin allowlist 但没有显式列表时，从确定的 TCP listener 推导唯一同源值。
/// Unix listener 和未指定地址没有可安全推导的浏览器 origin，必须由部署者显式填写。
pub(crate) fn resolve_http_security_origins(
    config: &mut HttpSecurityConfig,
    listen_addr: Option<SocketAddr>,
) -> Result<(), ConfigError> {
    if !config.allowed_origins_enabled || !config.allowed_origins.is_empty() {
        return Ok(());
    }
    let address = listen_addr.ok_or(ConfigError::InvalidHttpSecurity(
        "启用 allowed_origins 时，Unix listener 必须显式配置 allowed_origins",
    ))?;
    if address.ip().is_unspecified() {
        return Err(ConfigError::InvalidHttpSecurity(
            "未指定 TCP listener 无法推导浏览器 origin，必须显式配置 allowed_origins",
        ));
    }
    let scheme = if config.cookie_secure { "https" } else { "http" };
    // 与显式来源校验使用同一 URL 规范化规则：默认 80/443 不保留端口，IPv6 与
    // 非默认端口由 URL origin 序列化处理，避免自动推导出随后会被自身拒绝的来源。
    let origin = Url::parse(&format!("{scheme}://{address}"))
        .map_err(|_| ConfigError::InvalidHttpSecurity("无法从 TCP listener 推导浏览器 origin"))?
        .origin()
        .ascii_serialization();
    config.allowed_origins.push(origin);
    Ok(())
}

/// Public/Admin 与旧版保持一致：metrics 只能通过 loopback TCP 或 Unix listener 暴露。
pub(crate) fn validate_metrics_listener(
    role: Role,
    metrics: MetricsConfig,
    uses_unix_listener: bool,
    listen_addr: Option<SocketAddr>,
) -> Result<(), ConfigError> {
    if !metrics.enabled || role == Role::Worker || uses_unix_listener {
        return Ok(());
    }
    if listen_addr.is_some_and(|address| address.ip().is_loopback()) {
        return Ok(());
    }
    Err(ConfigError::InvalidMetricsConfig(
        "Public/Admin 启用 metrics 时必须使用 loopback TCP 或 Unix listener",
    ))
}

pub(crate) fn validate_realtime(config: &RealtimeConfig) -> Result<(), ConfigError> {
    if config.consumer_key.trim().is_empty()
        || config.consumer_key != config.consumer_key.trim()
        || config.consumer_key.len() > 128
        || config.consumer_key.chars().any(char::is_control)
    {
        return Err(ConfigError::InvalidRealtimeConfig(
            "consumer_key 必须是 1 至 128 字节的无控制字符标识",
        ));
    }
    if config.poll_interval_millis == 0 || config.poll_interval_millis > 60_000 {
        return Err(ConfigError::InvalidRealtimeConfig("poll_interval_millis 必须位于 1 至 60000"));
    }
    if config.stream_batch_size == 0 || config.stream_batch_size > 200 {
        return Err(ConfigError::InvalidRealtimeConfig("stream_batch_size 必须位于 1 至 200"));
    }
    if config.event_batch_size == 0 || config.event_batch_size > 200 {
        return Err(ConfigError::InvalidRealtimeConfig("event_batch_size 必须位于 1 至 200"));
    }
    if config.client_buffer == 0 || config.client_buffer > 1024 {
        return Err(ConfigError::InvalidRealtimeConfig("client_buffer 必须位于 1 至 1024"));
    }
    Ok(())
}

/// Public 只验证并发上限；Admin/Worker 沿用原有双重时限验证。
pub(crate) fn validate_role_session(config: &RoleSessionConfig) -> Result<(), ConfigError> {
    match config {
        RoleSessionConfig::Public(config) => {
            validate_max_public_sessions(config.max_public_sessions)
        }
        RoleSessionConfig::Timed(config) => validate_session(config),
    }
}

pub(crate) fn validate_session(config: &SessionConfig) -> Result<(), ConfigError> {
    if !(60..=24 * 60 * 60).contains(&config.idle_ttl_seconds) {
        return Err(ConfigError::InvalidSessionConfig("idle_ttl_seconds 必须位于 60 至 86400"));
    }
    if config.absolute_ttl_seconds < config.idle_ttl_seconds
        || config.absolute_ttl_seconds > 30 * 24 * 60 * 60
    {
        return Err(ConfigError::InvalidSessionConfig(
            "absolute_ttl_seconds 必须不小于 idle_ttl_seconds 且不大于 2592000",
        ));
    }
    validate_max_public_sessions(config.max_public_sessions)
}

fn validate_max_public_sessions(max_public_sessions: u32) -> Result<(), ConfigError> {
    if max_public_sessions == 0 || max_public_sessions > 100 {
        return Err(ConfigError::InvalidSessionConfig("max_public_sessions 必须位于 1 至 100"));
    }
    Ok(())
}
