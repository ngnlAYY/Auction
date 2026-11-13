//! runtime 文档各区块的边界校验。

use std::{collections::HashSet, path::Path};

use ipnet::IpNet;

use super::{
    business::{
        AdminRuntimeEquipmentFilterConfig, AdminRuntimeWorldConfig, AdminRuntimeWorldsConfig,
    },
    document::{
        AdminRuntimeDashboardMailbox, AdminRuntimeDocument, AdminRuntimeEquipmentSourceConfig,
        AdminRuntimeListenMode, AdminRuntimeListenerConfig, AdminRuntimeSecurityConfig,
        AdminRuntimeSessionConfig, AdminRuntimeTracingConfig,
    },
    errors::AdminRuntimeConfigError,
};
pub(super) use auction_domain::valid_text;

pub(super) fn validate_business_config(
    document: &AdminRuntimeDocument,
) -> Result<(), AdminRuntimeConfigError> {
    match (&document.worlds, &document.equipment_filter, &document.mail_templates) {
        (None, None, None) => Ok(()),
        (Some(worlds), Some(equipment_filter), Some(mail_templates)) => {
            validate_worlds(worlds)?;
            validate_runtime_equipment_filter(equipment_filter)?;
            mail_templates.validate().map_err(|(_key, error)| {
                AdminRuntimeConfigError::InvalidDocument(error.message().to_owned())
            })
        }
        _ => Err(AdminRuntimeConfigError::InvalidDocument(
            "worlds、equipment_filter 与 mail_templates 必须同时配置".to_owned(),
        )),
    }
}

fn validate_worlds(config: &AdminRuntimeWorldsConfig) -> Result<(), AdminRuntimeConfigError> {
    if !config.main.enabled && !config.isekai.enabled {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "worlds 至少必须启用 main 或 isekai".to_owned(),
        ));
    }
    validate_runtime_world("main", &config.main)?;
    validate_runtime_world("isekai", &config.isekai)
}

fn validate_runtime_world(
    name: &str,
    config: &AdminRuntimeWorldConfig,
) -> Result<(), AdminRuntimeConfigError> {
    if config.sender_allowlist.len() > 1000 {
        return Err(AdminRuntimeConfigError::InvalidDocument(format!(
            "worlds.{name}.sender_allowlist 最多 1000 项"
        )));
    }
    let mut senders = HashSet::with_capacity(config.sender_allowlist.len());
    for sender in &config.sender_allowlist {
        if !valid_runtime_identity(sender) || !senders.insert(sender.as_str()) {
            return Err(AdminRuntimeConfigError::InvalidDocument(format!(
                "worlds.{name}.sender_allowlist 含有重复或无效身份"
            )));
        }
    }
    Ok(())
}

fn validate_runtime_equipment_filter(
    config: &AdminRuntimeEquipmentFilterConfig,
) -> Result<(), AdminRuntimeConfigError> {
    if !(0..=1000).contains(&config.level.min)
        || !(0..=1000).contains(&config.level.max)
        || config.level.max != 0 && config.level.max < config.level.min
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "equipment_filter.level 必须位于 0 至 1000；0 表示不限制，非零 max 不能小于 min"
                .to_owned(),
        ));
    }
    let lists = [
        &config.qualities,
        &config.prefixes,
        &config.suffixes,
        &config.categories,
        &config.equipment_types,
    ];
    for list in lists {
        if list.len() > 256 {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "equipment_filter 每个 token 列表最多 256 项".to_owned(),
            ));
        }
        let mut seen = HashSet::with_capacity(list.len());
        for value in list {
            if !valid_runtime_token(value) || !seen.insert(value.as_str()) {
                return Err(AdminRuntimeConfigError::InvalidDocument(
                    "equipment_filter token 必须是唯一的 1 至 128 字节单行文本".to_owned(),
                ));
            }
        }
    }
    // 品质门槛始终存在（空列表也回退默认 L/P），因此无需其它过滤维度。
    Ok(())
}

fn valid_runtime_identity(value: &str) -> bool {
    auction_domain::valid_text(value, 128)
}

fn valid_runtime_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value == value.trim()
        && !value.chars().any(|character| matches!(character, '\0' | '\r' | '\n'))
}

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
                || !valid_listener_group(&config.group)
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
    for (index, mailbox) in mailboxes.iter().enumerate() {
        if !matches!(mailbox.world_id.as_str(), "main" | "isekai")
            || !valid_text(&mailbox.mailbox_key, 256)
            || mailboxes[..index].iter().any(|previous| {
                previous.world_id == mailbox.world_id || previous.mailbox_key == mailbox.mailbox_key
            })
        {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "dashboard_mailboxes 的 world_id/mailbox_key 无效或重复".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_equipment_source(
    config: &AdminRuntimeEquipmentSourceConfig,
) -> Result<(), AdminRuntimeConfigError> {
    if config.allowed_hosts.len() > 16
        || config.allowed_path_prefixes.len() > 16
        || !(64 << 10..=8 << 20).contains(&config.maximum_response_bytes)
        || !(1..=60_000).contains(&config.minimum_request_interval_millis)
        || !(1..=30).contains(&config.request_timeout_seconds)
        || config.allowlist_enabled && config.allowed_hosts.is_empty()
        || config.allowlist_enabled && config.allowed_path_prefixes.is_empty()
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "equipment_source 边界无效".to_owned(),
        ));
    }
    if config.allowed_hosts.iter().enumerate().any(|(index, host)| {
        !valid_allowlist_host(host) || config.allowed_hosts[..index].contains(host)
    }) || config.allowed_path_prefixes.iter().enumerate().any(|(index, prefix)| {
        !valid_allowlist_path_prefix(prefix)
            || config.allowed_path_prefixes[..index].contains(prefix)
    }) {
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
        && config.allowed_origins.iter().any(|origin| !valid_origin(origin))
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
        if network.prefix_len() == 0
            || network.trunc().to_string() != *value
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
    if !auction_domain::valid_text(value, 512)
        || !value.starts_with('/')
        || value == "/"
        || value.contains("//")
        || value.split('/').any(|segment| segment == "." || segment == "..")
    {
        return Err(AdminRuntimeConfigError::InvalidDocument(format!("{field} 无效")));
    }
    Ok(())
}

/// Unix 监听 socket 的属组名：1 至 64 字节，字母数字开头，其后只含字母数字与 `._-`；
/// Admin runtime 与部署配置共用。
pub fn valid_listener_group(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    value.len() <= 64
        && first.is_ascii_alphanumeric()
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
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

/// 装备来源 allowlist 的主机名：小写 DNS 名称，拒绝 IP 字面量；Admin runtime 与 Worker 适配器共用。
pub fn valid_allowlist_host(value: &str) -> bool {
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

/// 装备来源 allowlist 的路径前缀：`/` 开头结尾，拒绝点段、查询、片段与转义。
pub fn valid_allowlist_path_prefix(value: &str) -> bool {
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
        return valid_allowlist_host(authority);
    };
    !hostname.contains(':')
        && (valid_allowlist_host(hostname) || hostname.parse::<std::net::IpAddr>().is_ok())
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

/// bot 显示名必须是非空单行文本，最多 16 个且大小写不重复。
pub(super) fn validate_bot_players(players: &[String]) -> Result<(), AdminRuntimeConfigError> {
    if players.len() > 16 {
        return Err(AdminRuntimeConfigError::InvalidDocument(
            "bot_players 最多允许 16 个显示名".to_owned(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for player in players {
        let trimmed = player.trim();
        if trimmed.is_empty()
            || trimmed.len() > 64
            || trimmed != player
            || player.chars().any(char::is_control)
        {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "bot_players 显示名必须是 1-64 字节的单行文本且不含首尾空白".to_owned(),
            ));
        }
        if !seen.insert(trimmed.to_ascii_lowercase()) {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "bot_players 显示名不能大小写重复".to_owned(),
            ));
        }
    }
    Ok(())
}
