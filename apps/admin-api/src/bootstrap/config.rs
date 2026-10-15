//! Admin API 的普通配置、Worker control 和 dashboard 范围投影。

use std::{sync::Arc, time::Duration};

use anyhow::Context;
#[cfg(feature = "postgres")]
use auction_application::AdminIntakePolicy;
use auction_application::{AdminCredentialWorld, WorkerControlPort};
use auction_cache::CacheStore;
use auction_config::{AppConfig, CacheBackend};
use auction_http::{AdminHttpSecurity, HttpSecurityOptions};
#[cfg(feature = "postgres")]
use auction_integrations::EquipmentSourcePolicy;
use auction_jobs::WorkerControlClient;

pub(super) fn configure_worker_control(config: &AppConfig) -> Option<Arc<dyn WorkerControlPort>> {
    match WorkerControlClient::new(config.worker_control_socket_path(), Duration::from_secs(5)) {
        Ok(client) => Some(Arc::new(client)),
        Err(error) => {
            tracing::warn!(error = %error, "Worker control client configuration is unavailable");
            None
        }
    }
}

/// 将 Admin runtime 声明的 Worker world 配置投影到 dashboard；未声明时不强制假设 world。
pub(super) fn configured_dashboard_worlds(config: &AppConfig) -> Vec<AdminCredentialWorld> {
    configured_dashboard_mailboxes(config)
        .into_iter()
        .filter_map(|(world_id, _)| AdminCredentialWorld::parse(&world_id))
        .collect()
}

/// 将 Admin runtime 声明的邮箱标识投影到 Admin dashboard，保留自定义 mailbox_key 语义。
pub(super) fn configured_dashboard_mailbox_keys(config: &AppConfig) -> Vec<String> {
    configured_dashboard_mailboxes(config).into_iter().map(|(_, mailbox_key)| mailbox_key).collect()
}

/// 返回 Admin runtime 明确声明的 dashboard 邮箱范围；兼容程序化装配时的 Worker 配置投影。
pub(super) fn configured_dashboard_mailboxes(config: &AppConfig) -> Vec<(String, String)> {
    if let Some(runtime) = config.admin_runtime.as_ref()
        && !runtime.dashboard_mailboxes.is_empty()
    {
        return runtime
            .dashboard_mailboxes
            .iter()
            .map(|mailbox| (mailbox.world_id.clone(), mailbox.mailbox_key.clone()))
            .collect();
    }
    if !config.worker.mooglemail.enabled {
        return Vec::new();
    }

    config
        .worker
        .mooglemail
        .configured_worlds()
        .into_iter()
        .map(|world| (world.world_id, world.mailbox_key))
        .collect()
}

#[cfg(feature = "postgres")]
pub(super) fn equipment_source_policy(config: &AppConfig) -> anyhow::Result<EquipmentSourcePolicy> {
    let source = config
        .admin_runtime
        .as_ref()
        .map(|runtime| runtime.equipment_source.clone())
        .unwrap_or_default();
    Ok(EquipmentSourcePolicy {
        allowed_hosts: source.allowed_hosts,
        allowed_path_prefixes: source.allowed_path_prefixes,
        allowlist_enabled: source.allowlist_enabled,
        maximum_response_bytes: usize::try_from(source.maximum_response_bytes)
            .context("admin equipment_source response limit exceeds this platform")?,
        minimum_request_interval: Duration::from_millis(source.minimum_request_interval_millis),
        request_timeout: Duration::from_secs(source.request_timeout_seconds),
    })
}

#[cfg(feature = "postgres")]
pub(super) fn admin_intake_policy(config: &AppConfig) -> AdminIntakePolicy {
    let require_tradeable = config
        .admin_runtime
        .as_ref()
        .is_some_and(|runtime| runtime.equipment_source.require_tradeable);
    AdminIntakePolicy::new(require_tradeable)
}

pub(super) fn configure_security(config: &AppConfig) -> anyhow::Result<AdminHttpSecurity> {
    AdminHttpSecurity::new(HttpSecurityOptions {
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
