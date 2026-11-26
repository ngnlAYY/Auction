//! Public API 的普通配置和安全装配。

use std::time::Duration;

use anyhow::Context;
use auction_application::consignment::{ListingRegistrationPolicy, ListingWorlds};
use auction_config::{AppConfig, ConsignmentConfig};
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
            .context("Public 匿名读并发上限尚未解析")?,
        rate_limit_window: Duration::from_secs(config.http_security.rate_limit_window_seconds),
        request_timeout: Duration::from_secs(config.http_security.request_timeout_seconds),
        trusted_proxy_cidrs: &config.http_security.trusted_proxy_cidrs,
        trusted_unix_proxy: config.http_security.trusted_unix_proxy,
    })
    .and_then(|security| {
        security.with_anonymous_listing_limit(config.http_security.anonymous_listing_rate_limit)
    })
    .map_err(anyhow::Error::msg)
}

/// 由已校验的寄售配置生成登记准入策略；各额度收窄失败时给出可定位的启动错误。
pub(super) fn registration_policy(
    config: &ConsignmentConfig,
) -> anyhow::Result<ListingRegistrationPolicy> {
    Ok(ListingRegistrationPolicy {
        enabled: config.enabled(),
        // 决定 4 对两世界开放，寄售节没有独立世界配置。实际准入由数据库的世界可用性再收紧。
        worlds: ListingWorlds { main: true, isekai: true },
        consigned_fee_bps: config.fee_bps(),
        max_open_per_user: config.max_open_per_user().try_into().context("寄售活动挂单额度无效")?,
        registrations_per_hour: config
            .registrations_per_hour()
            .try_into()
            .context("寄售每小时登记额度无效")?,
        anonymous_resolving_limit: config
            .anonymous_resolving_limit()
            .try_into()
            .context("免登录解析中挂单上限无效")?,
    })
}
