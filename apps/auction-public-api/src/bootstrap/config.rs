//! Public API 的普通配置和安全装配。

use std::time::Duration;

use anyhow::Context;
use auction_config::AppConfig;
use auction_http::{HttpSecurityOptions, PublicHttpSecurity};

pub(super) fn configure_security(config: &AppConfig) -> anyhow::Result<PublicHttpSecurity> {
    PublicHttpSecurity::new(HttpSecurityOptions {
        cookie_secure: config.http_security.cookie_secure,
        origins_enabled: config.http_security.allowed_origins_enabled,
        allowed_origins: &config.http_security.allowed_origins,
        login_limit: config.http_security.login_rate_limit,
        mutation_limit: config.http_security.mutation_rate_limit,
        anonymous_read_limit: config.http_security.anonymous_read_rate_limit,
        // Public 配置加载已按服务池解析为显式值；缺失说明装配路径不完整，直接失败。
        anonymous_read_concurrency: config
            .http_security
            .anonymous_read_concurrency
            .context("public anonymous read concurrency is not resolved")?,
        rate_limit_window: Duration::from_secs(config.http_security.rate_limit_window_seconds),
        request_timeout: Duration::from_secs(config.http_security.request_timeout_seconds),
        trusted_proxy_cidrs: &config.http_security.trusted_proxy_cidrs,
        trusted_unix_proxy: config.http_security.trusted_unix_proxy,
    })
    .map_err(anyhow::Error::msg)
}
