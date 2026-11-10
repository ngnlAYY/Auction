//! Admin API 的普通配置、Worker control 和 dashboard 范围投影。

use std::{sync::Arc, time::Duration};

use auction_application::admin::AdminCredentialWorld;
use auction_application::admin::AdminIntakePolicy;
use auction_application::operations::WorkerControlPort;
use auction_config::AppConfig;
use auction_http::{AdminHttpSecurity, HttpSecurityOptions};
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
        .filter_map(|(world_id, _)| AdminCredentialWorld::parse(world_id))
        .collect()
}

/// 将 Admin runtime 声明的邮箱标识投影到 Admin dashboard，保留自定义 mailbox_key 语义。
pub(super) fn configured_dashboard_mailbox_keys(config: &AppConfig) -> Vec<String> {
    configured_dashboard_mailboxes(config).map(|(_, mailbox_key)| mailbox_key.to_owned()).collect()
}

/// 借用 Admin runtime 的 dashboard 邮箱范围；未声明时仅回退已启用的 Worker 配置。
fn configured_dashboard_mailboxes(config: &AppConfig) -> impl Iterator<Item = (&str, &str)> {
    let runtime_mailboxes = config
        .admin_runtime
        .as_ref()
        .map(|runtime| runtime.dashboard_mailboxes.as_slice())
        .filter(|mailboxes| !mailboxes.is_empty());
    let worker_mailboxes = (runtime_mailboxes.is_none() && config.worker.mooglemail.enabled)
        .then_some(&config.worker.mooglemail);
    runtime_mailboxes
        .into_iter()
        .flatten()
        .map(|mailbox| (mailbox.world_id.as_str(), mailbox.mailbox_key.as_str()))
        .chain(worker_mailboxes.into_iter().flat_map(|mail| mail.configured_mailboxes()))
}

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
        // Admin 路由不挂载匿名读闸门，这里仅满足共享构造签名。
        anonymous_read_limit: config.http_security.anonymous_read_rate_limit,
        anonymous_read_concurrency: config.http_security.anonymous_read_concurrency.unwrap_or(1),
        rate_limit_window: Duration::from_secs(config.http_security.rate_limit_window_seconds),
        request_timeout: Duration::from_secs(config.http_security.request_timeout_seconds),
        trusted_proxy_cidrs: &config.http_security.trusted_proxy_cidrs,
        trusted_unix_proxy: config.http_security.trusted_unix_proxy,
    })
    .map_err(anyhow::Error::msg)
}
