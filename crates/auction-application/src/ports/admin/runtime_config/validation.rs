use std::path::Path;

use ipnet::IpNet;

use super::{
    document::{
        AdminRuntimeDashboardMailbox, AdminRuntimeEquipmentSourceConfig, AdminRuntimeListenMode,
        AdminRuntimeListenerConfig, AdminRuntimeSecurityConfig, AdminRuntimeSessionConfig,
        AdminRuntimeTracingConfig,
    },
    errors::AdminRuntimeConfigError,
};

pub(super) fn validate_listener(
    config: &AdminRuntimeListenerConfig,
) -> Result<(), AdminRuntimeConfigError> {
    if (!config.address.is_empty() && !valid_text(&config.address, 256))
        || (!config.group.is_empty() && !valid_text(&config.group, 128))
        || (!config.runtime_directory.is_empty() && !valid_text(&config.runtime_directory, 512))
        || (!config.socket_path.is_empty() && !valid_text(&config.socket_path, 512))
    {
        return Err(AdminRuntimeConfigError::InvalidDocument("listener 字段无效".to_owned()));
    }
    match config.mode {
        AdminRuntimeListenMode::Tcp => {
            if !valid_text(&config.address, 256)
                || !config.group.is_empty()
                || !config.socket_path.is_empty()
                || config.address.parse::<std::net::SocketAddr>().is_err()
            {
                return Err(AdminRuntimeConfigError::InvalidDocument(
                    "TCP listener 字段无效".to_owned(),
                ));
            }
        }
        AdminRuntimeListenMode::Unix => {
            if !config.address.is_empty()
                || !valid_group_name(&config.group)
                || validate_unix_path(&config.runtime_directory, "listener.runtime_directory")
                    .is_err()
                || validate_unix_path(&config.socket_path, "listener.socket_path").is_err()
                || Path::new(&config.socket_path).parent()
                    != Some(Path::new(&config.runtime_directory))
            {
                return Err(AdminRuntimeConfigError::InvalidDocument(
                    "Unix listener 必须提供规范化的 runtime_directory、socket_path 和 group"
                        .to_owned(),
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_dashboard_mailboxes(
    mailboxes: &[AdminRuntimeDashboardMailbox],
) -> Result<(), AdminRuntimeConfigError> {
    if mailboxes.len() > 2 {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "dashboard_mailboxes 最多只能配置两个 world".to_owned(),
        ));
    }
    let mut world_ids = Vec::with_capacity(mailboxes.len());
    let mut mailbox_keys = Vec::with_capacity(mailboxes.len());
    for mailbox in mailboxes {
        if !matches!(mailbox.world_id.as_str(), "main" | "isekai")
            || !valid_text(&mailbox.mailbox_key, 256)
            || world_ids.iter().any(|world| world == &mailbox.world_id)
            || mailbox_keys.iter().any(|key| key == &mailbox.mailbox_key)
        {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "dashboard_mailboxes 的 world_id/mailbox_key 无效或重复".to_owned(),
            ));
        }
        world_ids.push(mailbox.world_id.clone());
        mailbox_keys.push(mailbox.mailbox_key.clone());
    }
    Ok(())
}

pub(super) fn validate_equipment_source(
    config: &AdminRuntimeEquipmentSourceConfig,
) -> Result<(), AdminRuntimeConfigError> {
    if config.allowed_hosts.len() > 16
        || config.allowed_path_prefixes.len() > 16
        || !(64 << 10..=8 << 20).contains(&config.maximum_response_bytes)
        || !(1..=60).contains(&config.minimum_request_interval_seconds)
        || !(1..=30).contains(&config.request_timeout_seconds)
        || config.allowlist_enabled && config.allowed_hosts.is_empty()
        || config.allowlist_enabled && config.allowed_path_prefixes.is_empty()
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "equipment_source 边界无效".to_owned(),
        ));
    }
    if config
        .allowed_hosts
        .iter()
        .enumerate()
        .any(|(index, host)| !valid_dns_name(host) || config.allowed_hosts[..index].contains(host))
        || config.allowed_path_prefixes.iter().enumerate().any(|(index, prefix)| {
            !valid_path_prefix(prefix) || config.allowed_path_prefixes[..index].contains(prefix)
        })
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "equipment_source allowlist 格式无效".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_security(
    config: &AdminRuntimeSecurityConfig,
) -> Result<(), AdminRuntimeConfigError> {
    if config.login_rate_limit == 0
        || config.login_rate_limit > 10_000
        || config.mutation_rate_limit == 0
        || config.mutation_rate_limit > 10_000
        || !(1..=3600).contains(&config.rate_limit_window_seconds)
        || !(1..=120).contains(&config.request_timeout_seconds)
        || config.allowed_origins.len() > 16
        || config.trusted_proxy_cidrs.len() > 64
    {
        return Err(AdminRuntimeConfigError::InvalidDocument("security 边界无效".to_owned()));
    }
    if config.allowed_origins_enabled
        && (config.allowed_origins.is_empty()
            || config.allowed_origins.iter().any(|origin| !valid_origin(origin)))
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "security.allowed_origins 无效".to_owned(),
        ));
    }
    for (index, value) in config.trusted_proxy_cidrs.iter().enumerate() {
        let Ok(network) = value.parse::<IpNet>() else {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "security.trusted_proxy_cidrs 无效".to_owned(),
            ));
        };
        if network.trunc().to_string() != *value
            || config.trusted_proxy_cidrs[..index].iter().any(|existing| existing == value)
        {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "security.trusted_proxy_cidrs 必须规范且不能重复".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_session(
    config: &AdminRuntimeSessionConfig,
) -> Result<(), AdminRuntimeConfigError> {
    if !(60..=86_400).contains(&config.idle_ttl_seconds)
        || config.absolute_ttl_seconds < config.idle_ttl_seconds
        || config.absolute_ttl_seconds > 2_592_000
        || config.max_public_sessions == 0
        || config.max_public_sessions > 100
    {
        return Err(AdminRuntimeConfigError::InvalidDocument("session 边界无效".to_owned()));
    }
    Ok(())
}

pub(super) fn validate_tracing(
    config: &AdminRuntimeTracingConfig,
) -> Result<(), AdminRuntimeConfigError> {
    if !(1..=30).contains(&config.batch_timeout_seconds)
        || !(1..=30).contains(&config.export_timeout_seconds)
        || !(1..=30).contains(&config.shutdown_timeout_seconds)
        || !(1..=512).contains(&config.max_export_batch_size)
        || !(1..=4096).contains(&config.queue_capacity)
        || config.max_export_batch_size > config.queue_capacity
        || config.sample_ratio_ppm > 1_000_000
        || config.enabled && !valid_http_url(&config.endpoint)
        || !config.enabled && !config.endpoint.is_empty() && !valid_http_url(&config.endpoint)
    {
        return Err(AdminRuntimeConfigError::InvalidDocument("tracing 边界无效".to_owned()));
    }
    Ok(())
}

pub(super) fn validate_unix_path(value: &str, field: &str) -> Result<(), AdminRuntimeConfigError> {
    if value.is_empty()
        || value.len() > 512
        || !value.starts_with('/')
        || value == "/"
        || value != value.trim()
        || value.chars().any(char::is_control)
        || value.contains("//")
        || value.split('/').any(|segment| segment == "." || segment == "..")
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(format!("{field} 无效")));
    }
    Ok(())
}

pub(super) fn valid_group_name(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    value.len() <= 64
        && first.is_ascii_alphanumeric()
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(super) fn valid_fingerprint(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].chars().all(|character| character.is_ascii_hexdigit())
        && value == value.to_ascii_lowercase()
}

pub(super) fn valid_unknown_category(value: &str) -> bool {
    matches!(
        value,
        "current_unavailable"
            | "current_corrupt"
            | "replace_unknown"
            | "result_corrupt"
            | "file_conflict"
    )
}

fn valid_dns_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value == value.to_ascii_lowercase()
        && value.parse::<std::net::IpAddr>().is_err()
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

fn valid_path_prefix(value: &str) -> bool {
    !value.is_empty()
        && value.starts_with('/')
        && value.ends_with('/')
        && value == value.trim()
        && !value.contains("//")
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '?' | '#' | '%' | '\\'))
        && value.split('/').all(|segment| segment != "." && segment != "..")
}

fn valid_origin(value: &str) -> bool {
    let Some((scheme, authority_and_path)) = value.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https") || authority_and_path.is_empty() {
        return false;
    }
    let (authority, path) = authority_and_path
        .split_once('/')
        .map_or((authority_and_path, None), |(authority, path)| (authority, Some(path)));
    if authority.is_empty()
        || authority != authority.trim()
        || path.is_some_and(|value| !value.is_empty())
        || authority.chars().any(|character| matches!(character, '?' | '#' | '@'))
    {
        return false;
    }
    if authority.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, suffix)) = bracketed.split_once(']') else {
            return false;
        };
        return host.parse::<std::net::IpAddr>().is_ok()
            && (suffix.is_empty()
                || suffix
                    .strip_prefix(':')
                    .is_some_and(|port| !port.is_empty() && port.parse::<u16>().is_ok()));
    }
    let Some((hostname, port)) = authority.rsplit_once(':') else {
        return valid_dns_name(authority);
    };
    !hostname.contains(':')
        && (valid_dns_name(hostname) || hostname.parse::<std::net::IpAddr>().is_ok())
        && !port.is_empty()
        && port.parse::<u16>().is_ok()
}

fn valid_http_url(value: &str) -> bool {
    (value.starts_with("http://") || value.starts_with("https://"))
        && !value
            .chars()
            .any(|character| matches!(character, ' ' | '\n' | '\r' | '\0' | '?' | '#' | '@'))
        && value.split_once("://").is_some_and(|(_, host)| !host.is_empty())
}
