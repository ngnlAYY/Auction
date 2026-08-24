//! Public API 的普通配置和安全/cache 装配。

use std::time::Duration;

use anyhow::Context;
use auction_cache::CacheStore;
use auction_config::{AppConfig, CacheBackend};
use auction_http::{HttpSecurityOptions, PublicHttpSecurity};

pub(super) fn configure_security(config: &AppConfig) -> anyhow::Result<PublicHttpSecurity> {
    PublicHttpSecurity::new(HttpSecurityOptions {
        cookie_secure: config.http_security.cookie_secure,
        origins_enabled: config.http_security.allowed_origins_enabled,
        allowed_origins: &config.http_security.allowed_origins,
        login_limit: config.http_security.login_rate_limit,
        mutation_limit: config.http_security.mutation_rate_limit,
        rate_limit_window: Duration::from_secs(config.http_security.rate_limit_window_seconds),
        request_timeout: Duration::from_secs(config.http_security.request_timeout_seconds),
        trusted_proxy_cidrs: &config.http_security.trusted_proxy_cidrs,
        trusted_unix_proxy: config.http_security.trusted_unix_proxy,
    })
    .map_err(anyhow::Error::msg)
}

pub(super) fn configure_cache(config: &AppConfig) -> anyhow::Result<CacheStore> {
    match config.cache.backend {
        CacheBackend::Disabled => Ok(CacheStore::disabled()),
        CacheBackend::Redis => {
            let redis_url = config.redis_url().context("redis cache URL is missing")?;
            CacheStore::redis(redis_url).map_err(Into::into)
        }
    }
}
