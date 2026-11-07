//! 单角色有效配置的安全投影：只输出路径、开关、数量与非敏感策略，绝不输出连接串、
//! Cookie、数据密钥、OTLP headers 或 Admin 凭据。

use auction_config::{AppConfig, HttpListenMode, Role, RoleSessionConfig};
use auction_storage::DatabaseBackend;
use serde_json::{Value, json};

use super::ConfigDir;

pub(crate) fn show(config_dir: &ConfigDir, role: Role) -> anyhow::Result<()> {
    let config = config_dir.load(role)?;
    crate::app::print_json(&projection(&config)?)
}

fn projection(config: &AppConfig) -> anyhow::Result<Value> {
    let backend = config.database_url().map(DatabaseBackend::from_url).transpose()?;
    let listener = config.listener.as_ref().map(|listener| match listener.mode {
        HttpListenMode::Tcp => json!({
            "mode": listener.mode.name(),
            "address": config.listen_addr.map(|address| address.to_string()),
        }),
        HttpListenMode::Unix => json!({
            "mode": listener.mode.name(),
            "runtime_directory": listener.runtime_directory,
            "socket_path": listener.socket_path,
            "group": listener.group,
        }),
    });
    let session = match config.session {
        RoleSessionConfig::Public(session) => {
            json!({ "max_public_sessions": session.max_public_sessions })
        }
        RoleSessionConfig::Timed(session) => json!({
            "idle_ttl_seconds": session.idle_ttl_seconds,
            "absolute_ttl_seconds": session.absolute_ttl_seconds,
            "max_public_sessions": session.max_public_sessions,
        }),
    };
    let mail = &config.worker.mooglemail;
    let control = &config.worker.control;
    let forum = &config.worker.forum;
    let alerts = &config.worker.operations_alert;
    let schedule = &config.auction.schedule;
    let economics = &config.auction.economics;
    let settlement = &config.auction.settlement;
    let sender_allowlist_entries = mail.sender_allowlist.len()
        + mail.additional_worlds.iter().map(|world| world.sender_allowlist.len()).sum::<usize>();
    Ok(json!({
        "role": config.role.name(),
        "files": {
            "config": config.config_path.as_ref().map(|path| path.display().to_string()),
            "common": config.common_config_path.as_ref().map(|path| path.display().to_string()),
            "secrets_configured": config.secrets_path.is_some(),
            "runtime": config.runtime_config_path.as_ref().map(|path| path.display().to_string()),
            "worker": config.worker_config_path.as_ref().map(|path| path.display().to_string()),
        },
        "listener": listener,
        "database": {
            "configured": backend.is_some(),
            "backend": backend.map(DatabaseBackend::name),
            "profiles": config.database_profiles,
            "pool": config.database_pool,
            "effective_pool_max_connections": if config.database_profiles.mode
                == auction_config::DatabaseProfileMode::Legacy {
                16
            } else {
                config.database_pool.max_connections
            },
            "effective_acquire_timeout_millis": config.database_pool.timeouts()?
                .map_or(30_000, |value| value.acquire_timeout_millis),
            "restart_required": true,
            "application_name": if config.database_profiles.mode
                == auction_config::DatabaseProfileMode::Legacy {
                None
            } else {
                Some(format!("auction-{}", config.role.name()))
            },
        },
        "backup": backup_projection(config),
        "observability": {
            "log_filter": config.log_filter,
            "metrics_enabled": config.metrics.enabled,
            "tracing_enabled": config.tracing.enabled,
            "tracing_endpoint_configured": !config.tracing.endpoint.is_empty(),
            "tracing_headers_configured": config.tracing_headers().is_some(),
            "tracing_sample_ratio_ppm": config.tracing.sample_ratio_ppm,
        },
        "http_security": {
            "allowed_origins_enabled": config.http_security.allowed_origins_enabled,
            "allowed_origins": config.http_security.allowed_origins,
            "trusted_proxy_cidr_count": config.http_security.trusted_proxy_cidrs.len(),
            "trusted_unix_proxy": config.http_security.trusted_unix_proxy,
            "cookie_secure": config.http_security.cookie_secure,
            "request_timeout_seconds": config.http_security.request_timeout_seconds,
        },
        "session": session,
        "realtime": {
            "consumer_key": config.realtime.consumer_key,
            "poll_interval_millis": config.realtime.poll_interval_millis,
            "client_buffer": config.realtime.client_buffer,
        },
        "auction": {
            "schedule_enabled": schedule.enabled,
            "schedule_timezone": schedule.timezone,
            "schedule_worlds": schedule.worlds,
            "early_bird_enabled": schedule.early_bird.enabled,
            "start_price": economics.start_price,
            "bid_increment_ppm": economics.bid_increment_ppm,
            "soft_close_window_seconds": economics.soft_close_window_seconds,
            "soft_close_extension_seconds": economics.soft_close_extension_seconds,
            "proxy_enabled": economics.proxy_enabled,
            "settlement_version": settlement.version,
            "settlement_mode": settlement.mode,
        },
        "worker": {
            "concurrency": config.worker_concurrency(),
            "poll_interval_seconds": config.worker_poll_interval_seconds(),
            "control_enabled": control.enabled,
            "control_socket_path": control.socket_path,
            "control_allowed_peer_uid_count": control.allowed_peer_uids.len(),
            "control_allowed_peer_gid_count": control.allowed_peer_gids.len(),
            "mooglemail_enabled": mail.enabled,
            "mooglemail_worlds": mail.configured_mailboxes().map(|(world, _)| world).collect::<Vec<_>>(),
            "mooglemail_sender_allowlist_entries": sender_allowlist_entries,
            "mooglemail_diagnostics_enabled": mail.diagnostics.enabled,
            "forum_enabled": forum.enabled,
            "forum_credential_world": forum.credential_world,
            "operations_alert_enabled": alerts.enabled,
        },
        "bot_player_count": config.bot_players.len(),
        "equipment_color_rule_count": config.equipment_colors.len(),
        "admin_runtime_loaded": config.admin_runtime.is_some(),
        "keys": {
            "admin_credentials": config.admin_credentials_configured(),
            "equipment_data_key": config.equipment_data_key_id().is_some(),
            "user_login_data_key": config.user_login_data_key_id().is_some(),
            "credential_data_key": config.credential_data_key_configured(),
            "mail_diagnostics_data_key": config.mail_diagnostics_data_key_configured(),
            "retired_data_key_counts": {
                "equipment": config.data_key_previous(auction_config::DataKeyDomain::Equipment).len(),
                "user_login": config.data_key_previous(auction_config::DataKeyDomain::UserLogin).len(),
                "credential": config.data_key_previous(auction_config::DataKeyDomain::Credential).len(),
                "mail_diagnostics": config.data_key_previous(auction_config::DataKeyDomain::MailDiagnostics).len(),
            },
        },
    }))
}

pub(super) fn backup_projection(config: &AppConfig) -> Value {
    let offsite = &config.backup.offsite;
    json!({
        "local_retention_count": config.backup.local_retention_count,
        "min_free_bytes": config.backup.min_free_bytes,
        "min_free_percent": config.backup.min_free_percent,
        "timeout_seconds": config.backup.timeout_seconds,
        "heartbeat_success_configured": config.backup_heartbeat_success_url().is_some(),
        "heartbeat_failure_configured": config.backup_heartbeat_failure_url().is_some(),
        "scope": {
            "enabled": config.backup.scope.enabled,
            "runtime_config": config.backup.scope.runtime_config,
            "mail_diagnostics": config.backup.scope.mail_diagnostics,
            // 只投影是否配置，不输出部署回执目录的绝对路径。
            "deployment_receipts_configured":
                !config.backup.scope.deployment_receipts_dir.as_os_str().is_empty(),
            "max_files": config.backup.scope.max_files,
            "maximum_total_bytes": config.backup.scope.maximum_total_bytes,
        },
        "offsite": {
            "enabled": offsite.enabled,
            "age_recipient_configured": !offsite.age_recipient.is_empty(),
            "endpoint_configured": !offsite.endpoint.is_empty(),
            "bucket_configured": !offsite.bucket.is_empty(),
            "prefix_configured": !offsite.prefix.is_empty(),
            "access_key_id_configured": config.backup_r2_access_key_id().is_some(),
            "secret_access_key_configured": config.backup_r2_secret_access_key().is_some(),
        },
    })
}
