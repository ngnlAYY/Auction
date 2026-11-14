//! Admin runtime TOML 的读取与非敏感投影。

use std::path::Path;

use auction_application::admin::AdminRuntimeDocument;

use super::model::SecretFileConfig;
use crate::reader::read_toml_file;
use crate::runtime::{RuntimeFileStoreError, read_runtime_file};
use crate::settings::errors::ConfigError;
use crate::settings::validation::{
    resolve_http_security_origins, validate_http_listener, validate_http_security,
    validate_metrics_listener, validate_session, validate_tracing,
};
use crate::settings::{
    HttpListenMode, HttpListenerConfig, HttpSecurityConfig, MetricsConfig, Role, SessionConfig,
    TracingSettings,
};

/// 读取独立敏感 TOML；解析器拒绝未声明字段并固定脱敏错误。
pub(crate) fn load_secret_file(path: &Path) -> Result<SecretFileConfig, ConfigError> {
    let contents = read_toml_file(path)
        .map_err(|source| ConfigError::SecretRead { path: path.to_owned(), source })?;

    toml::from_str(&contents).map_err(|_| ConfigError::SecretParse { path: path.to_owned() })
}

/// 读取 Admin runtime TOML 并在进入进程配置前完成完整校验。
pub(crate) fn load_admin_runtime_file(path: &Path) -> Result<AdminRuntimeDocument, ConfigError> {
    let contents = read_runtime_file(path, None).map_err(|error| match error {
        RuntimeFileStoreError::Parse => ConfigError::RuntimeParse { path: path.to_owned() },
        RuntimeFileStoreError::Io(message) => ConfigError::RuntimeRead {
            path: path.to_owned(),
            source: std::io::Error::other(message),
        },
        error => {
            ConfigError::InvalidRuntimeConfig { path: path.to_owned(), reason: error.to_string() }
        }
    })?;
    let document: AdminRuntimeDocument = toml::from_str(&contents)
        .map_err(|_| ConfigError::RuntimeParse { path: path.to_owned() })?;
    crate::runtime::validate_runtime_document(&document, chrono::Utc::now()).map_err(|error| {
        ConfigError::InvalidRuntimeConfig { path: path.to_owned(), reason: error.to_string() }
    })?;
    Ok(document)
}

/// 将 Admin runtime 的 HTTP 部分投影为公共进程配置；不携带运行时密文。
pub(crate) fn runtime_http_security(document: &AdminRuntimeDocument) -> HttpSecurityConfig {
    HttpSecurityConfig::from(&document.security)
}

/// 将 Admin runtime 的 session 部分投影为公共进程配置。
pub(crate) fn runtime_session(document: &AdminRuntimeDocument) -> SessionConfig {
    SessionConfig {
        idle_ttl_seconds: document.session.idle_ttl_seconds,
        absolute_ttl_seconds: document.session.absolute_ttl_seconds,
        max_public_sessions: document.session.max_public_sessions,
    }
}

/// 将 runtime listener 投影为启动配置并复用统一校验。
pub(crate) fn admin_runtime_listener(
    runtime: &AdminRuntimeDocument,
) -> Result<HttpListenerConfig, ConfigError> {
    let listener = &runtime.listener;
    let effective = match listener.mode {
        auction_application::admin::AdminRuntimeListenMode::Tcp => HttpListenerConfig {
            address: listener.address.clone(),
            group: String::new(),
            mode: HttpListenMode::Tcp,
            runtime_directory: String::new(),
            socket_path: String::new(),
        },
        auction_application::admin::AdminRuntimeListenMode::Unix => HttpListenerConfig {
            address: String::new(),
            group: listener.group.clone(),
            mode: HttpListenMode::Unix,
            runtime_directory: listener.runtime_directory.clone(),
            socket_path: listener.socket_path.clone(),
        },
    };
    validate_http_listener(&effective)?;
    Ok(effective)
}

/// 保存和启动共用部署投影校验，避免应用层引入 URL 解析依赖。
pub(crate) fn validate_runtime_startup(document: &AdminRuntimeDocument) -> Result<(), ConfigError> {
    let listener = admin_runtime_listener(document)?;
    let uses_unix = listener.mode == HttpListenMode::Unix;
    let listen_addr = if uses_unix {
        None
    } else {
        Some(listener.address.parse().map_err(|source| ConfigError::InvalidAddress {
            field: "listener.address",
            value: listener.address.clone(),
            source,
        })?)
    };
    let mut security = runtime_http_security(document);
    resolve_http_security_origins(&mut security, listen_addr)?;
    validate_http_security(&security)?;
    if security.trusted_unix_proxy && !uses_unix {
        return Err(ConfigError::InvalidHttpSecurity(
            "trusted_unix_proxy 只能由 Public 或 Admin Unix listener 启用",
        ));
    }
    validate_metrics_listener(
        Role::Admin,
        MetricsConfig { enabled: document.metrics.enabled, ..MetricsConfig::default() },
        uses_unix,
        listen_addr,
    )?;
    validate_session(&runtime_session(document))?;
    validate_tracing(&runtime_tracing(document), None)
}

/// Admin 启动与保存验证使用同一 tracing 投影。
pub(crate) fn runtime_tracing(document: &AdminRuntimeDocument) -> TracingSettings {
    TracingSettings {
        batch_timeout_seconds: document.tracing.batch_timeout_seconds,
        enabled: document.tracing.enabled,
        endpoint: document.tracing.endpoint.clone(),
        export_timeout_seconds: document.tracing.export_timeout_seconds,
        max_export_batch_size: document.tracing.max_export_batch_size,
        queue_capacity: document.tracing.queue_capacity,
        sample_ratio_ppm: document.tracing.sample_ratio_ppm,
        shutdown_timeout_seconds: document.tracing.shutdown_timeout_seconds,
    }
}
