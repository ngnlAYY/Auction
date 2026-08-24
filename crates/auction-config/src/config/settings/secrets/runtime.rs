//! Admin runtime TOML 的读取与非敏感投影。

use std::{fs, path::Path};

use auction_application::AdminRuntimeDocument;

use super::super::{ConfigError, HttpSecurityConfig, SessionConfig};
use super::model::SecretFileConfig;

/// 读取独立敏感 TOML；解析器拒绝未声明字段并固定脱敏错误。
pub(crate) fn load_secret_file(path: &Path) -> Result<SecretFileConfig, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|source| ConfigError::SecretRead { path: path.to_owned(), source })?;

    toml::from_str(&contents).map_err(|_| ConfigError::SecretParse { path: path.to_owned() })
}

/// 读取 Admin runtime TOML 并在进入进程配置前完成完整校验。
pub(crate) fn load_admin_runtime_file(path: &Path) -> Result<AdminRuntimeDocument, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|source| ConfigError::RuntimeRead { path: path.to_owned(), source })?;
    let document: AdminRuntimeDocument = toml::from_str(&contents)
        .map_err(|_| ConfigError::RuntimeParse { path: path.to_owned() })?;
    document.validate().map_err(|error| ConfigError::InvalidRuntimeConfig {
        path: path.to_owned(),
        reason: error.to_string(),
    })?;
    Ok(document)
}

/// 将 Admin runtime 的 HTTP 部分投影为公共进程配置；不携带运行时密文。
pub(crate) fn runtime_http_security(document: &AdminRuntimeDocument) -> HttpSecurityConfig {
    HttpSecurityConfig {
        allowed_origins: document.security.allowed_origins.clone(),
        allowed_origins_enabled: document.security.allowed_origins_enabled,
        login_rate_limit: document.security.login_rate_limit,
        mutation_rate_limit: document.security.mutation_rate_limit,
        rate_limit_window_seconds: document.security.rate_limit_window_seconds,
        cookie_secure: document.security.cookie_secure,
        request_timeout_seconds: document.security.request_timeout_seconds,
        trusted_proxy_cidrs: document.security.trusted_proxy_cidrs.clone(),
        trusted_unix_proxy: document.security.trusted_unix_proxy,
    }
}

/// 将 Admin runtime 的 session 部分投影为公共进程配置。
pub(crate) fn runtime_session(document: &AdminRuntimeDocument) -> SessionConfig {
    SessionConfig {
        idle_ttl_seconds: document.session.idle_ttl_seconds,
        absolute_ttl_seconds: document.session.absolute_ttl_seconds,
        max_public_sessions: document.session.max_public_sessions,
    }
}
