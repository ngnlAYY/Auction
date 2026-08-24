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
        for origin in &config.allowed_origins {
            let parsed = Url::parse(origin).map_err(|_| {
                ConfigError::InvalidHttpSecurity("allowed_origins 必须是有效的 HTTP(S) origin")
            })?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || parsed.username() != ""
                || parsed.password().is_some()
                || parsed.path() != ""
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.as_str() != origin
            {
                return Err(ConfigError::InvalidHttpSecurity(
                    "allowed_origins 只能包含精确 scheme+host",
                ));
            }
        }
    }
    Ok(())
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
    if config.max_public_sessions == 0 || config.max_public_sessions > 100 {
        return Err(ConfigError::InvalidSessionConfig("max_public_sessions 必须位于 1 至 100"));
    }
    Ok(())
}
