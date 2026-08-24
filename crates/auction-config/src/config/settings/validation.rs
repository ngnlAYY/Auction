//! 基础配置、监听器、安全策略和路径解析校验。

use super::*;

pub(super) fn validate_auction(config: &AuctionConfig) -> Result<(), ConfigError> {
    if !matches!(config.settlement.mode.as_str(), "legacy_zero_v0" | "hold_curve_v1") {
        return Err(ConfigError::InvalidAuctionConfig(
            "settlement.mode 只能是 legacy_zero_v0 或 hold_curve_v1",
        ));
    }
    if config.settlement.recovery.mode != "remind_v1" {
        return Err(ConfigError::InvalidAuctionConfig("settlement.recovery.mode 只能是 remind_v1"));
    }
    config
        .policy()
        .validate()
        .map_err(|_| ConfigError::InvalidAuctionConfig("经济、结算或恢复字段边界无效"))
}

pub(super) fn resolve_http_listener(
    role: Role,
    listener: Option<HttpListenerConfig>,
    legacy_address: Option<String>,
) -> Result<Option<HttpListenerConfig>, ConfigError> {
    if listener.is_some() && legacy_address.is_some() {
        return Err(ConfigError::InvalidListenerConfig(
            "listener 与旧字段 listen_address 不能同时提供",
        ));
    }

    let mut listener = listener
        .or_else(|| legacy_address.map(HttpListenerConfig::tcp))
        .or_else(|| role.default_listen().map(HttpListenerConfig::tcp));
    if let Some(value) = listener.as_mut()
        && value.mode == HttpListenMode::Tcp
        && value.address.is_empty()
    {
        value.address = role.default_listen().unwrap_or_default().to_owned();
    }

    if role == Role::Worker {
        if listener.is_some() {
            return Err(ConfigError::InvalidListenerConfig("Worker 不允许配置 HTTP listener"));
        }
        return Ok(None);
    }

    let listener = listener
        .ok_or(ConfigError::InvalidListenerConfig("Public/Admin 必须提供有效的 HTTP listener"))?;
    validate_http_listener(&listener)?;
    Ok(Some(listener))
}

pub(super) fn validate_http_listener(config: &HttpListenerConfig) -> Result<(), ConfigError> {
    match config.mode {
        HttpListenMode::Tcp => {
            if !config.group.is_empty()
                || !config.runtime_directory.is_empty()
                || !config.socket_path.is_empty()
                || config.address.parse::<SocketAddr>().is_err()
            {
                return Err(ConfigError::InvalidListenerConfig(
                    "TCP listener 必须只提供有效的 address",
                ));
            }
        }
        HttpListenMode::Unix => {
            if !config.address.is_empty()
                || !valid_listener_group(&config.group)
                || !valid_listener_path(&config.runtime_directory)
                || !valid_listener_path(&config.socket_path)
                || Path::new(&config.socket_path).parent()
                    != Some(Path::new(&config.runtime_directory))
            {
                return Err(ConfigError::InvalidListenerConfig(
                    "Unix listener 必须提供规范化的 runtime_directory、socket_path 和 group",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn valid_listener_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.starts_with('/')
        && value != "/"
        && value == value.trim()
        && !value.contains("//")
        && !value.chars().any(char::is_control)
        && !value.split('/').any(|segment| segment == "." || segment == "..")
}

pub(super) fn valid_listener_group(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    value.len() <= 64
        && first.is_ascii_alphanumeric()
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

pub(super) fn validate_cache(config: &CacheConfig) -> Result<(), ConfigError> {
    if !(1..=3600).contains(&config.public_rules_ttl_seconds) {
        return Err(ConfigError::InvalidCacheConfig("public_rules_ttl_seconds 必须位于 1 至 3600"));
    }
    Ok(())
}

pub(super) fn validate_http_security(config: &HttpSecurityConfig) -> Result<(), ConfigError> {
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

pub(super) fn validate_realtime(config: &RealtimeConfig) -> Result<(), ConfigError> {
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

pub(super) fn validate_tracing(
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

pub(super) fn validate_tracing_headers(value: &str) -> Result<(), ConfigError> {
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

pub(super) fn valid_http_header_token(value: &str) -> bool {
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

pub(super) fn validate_session(config: &SessionConfig) -> Result<(), ConfigError> {
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

pub(super) fn resolve_secrets_path(config_path: Option<&Path>, requested_path: PathBuf) -> PathBuf {
    if requested_path.is_absolute() {
        return requested_path;
    }

    config_path.and_then(Path::parent).unwrap_or_else(|| Path::new(".")).join(requested_path)
}

pub(super) fn resolve_config_path(config_path: Option<&Path>, requested_path: PathBuf) -> PathBuf {
    if requested_path.is_absolute() {
        return requested_path;
    }

    config_path.and_then(Path::parent).unwrap_or_else(|| Path::new(".")).join(requested_path)
}

pub(super) fn resolve_worker_paths(
    config_path: Option<&Path>,
    mut worker: WorkerConfig,
) -> WorkerConfig {
    worker.mooglemail.keyenv_file =
        worker.mooglemail.keyenv_file.map(|path| resolve_config_path(config_path, path));
    worker
}

pub(super) fn database_url_is_sqlite(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("sqlite"))
}
