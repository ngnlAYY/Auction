//! Public/Admin HTTP listener 和 Unix runtime 路径校验。

use std::net::SocketAddr;
use std::path::Path;

use crate::settings::errors::ConfigError;
use crate::settings::{HttpListenMode, HttpListenerConfig, Role};

pub(crate) fn resolve_http_listener(
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

pub(crate) fn validate_http_listener(config: &HttpListenerConfig) -> Result<(), ConfigError> {
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
                || !auction_application::admin::valid_listener_group(&config.group)
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

pub(crate) fn valid_listener_path(value: &str) -> bool {
    auction_domain::valid_text(value, 512)
        && value.starts_with('/')
        && value != "/"
        && !value.contains("//")
        && !value.split('/').any(|segment| segment == "." || segment == "..")
}
