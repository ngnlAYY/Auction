//! 敏感快照、runtime 覆盖和全部配置不变量的最终构建。

use super::super::*;
use super::model::{AppConfig, BuildInput};

impl AppConfig {
    pub(super) fn build(role: Role, input: BuildInput) -> Result<Self, ConfigError> {
        let BuildInput {
            config_path,
            listen_address,
            listener,
            log_filter,
            metrics,
            tracing,
            realtime,
            cache,
            auction,
            worker,
            http_security,
            session,
            secrets_path,
            runtime_config_path,
        } = input;
        let secret_file =
            secrets_path.as_deref().map(load_secret_file).transpose()?.unwrap_or_default();
        let secrets = AppSecrets::from_file(secret_file)?;

        let admin_runtime = if role == Role::Admin {
            runtime_config_path.as_deref().map(load_admin_runtime_file).transpose()?
        } else {
            None
        };
        let listener = resolve_http_listener(role, listener, listen_address)?;

        // Admin runtime 文件是 Admin 运行时策略的唯一生效源；否则 metrics 区块虽然能被
        // 解析并通过 runtime API 返回，进程装配却仍会读取主配置，形成“可写但不生效”的假配置。
        let metrics = admin_runtime
            .as_ref()
            .map(|runtime| MetricsConfig { enabled: runtime.metrics.enabled })
            .unwrap_or(metrics);
        let tracing = admin_runtime
            .as_ref()
            .map(|runtime| TracingSettings {
                batch_timeout_seconds: runtime.tracing.batch_timeout_seconds,
                enabled: runtime.tracing.enabled,
                endpoint: runtime.tracing.endpoint.clone(),
                export_timeout_seconds: runtime.tracing.export_timeout_seconds,
                max_export_batch_size: runtime.tracing.max_export_batch_size,
                queue_capacity: runtime.tracing.queue_capacity,
                sample_ratio_ppm: runtime.tracing.sample_ratio_ppm,
                shutdown_timeout_seconds: runtime.tracing.shutdown_timeout_seconds,
            })
            .unwrap_or(tracing);

        if secrets.equipment_data_key_id.is_some() != secrets.equipment_data_key.is_some() {
            return Err(ConfigError::InvalidEquipmentDataKeyConfiguration);
        }
        if secrets.user_login_data_key_id.is_some() != secrets.user_login_data_key.is_some() {
            return Err(ConfigError::InvalidUserLoginDataKeyConfiguration);
        }
        if secrets.credential_data_key_id.is_some() != secrets.credential_data_key.is_some() {
            return Err(ConfigError::InvalidCredentialDataKeyConfiguration);
        }

        if cache.backend == CacheBackend::Redis && secrets.redis_url.is_none() {
            return Err(ConfigError::MissingRedisUrl);
        }
        validate_cache(&cache)?;
        let database_is_sqlite = secrets
            .database_url
            .as_ref()
            .is_some_and(|url| database_url_is_sqlite(url.expose_secret()));
        let worker_concurrency =
            worker.concurrency.unwrap_or(if database_is_sqlite { 1 } else { 8 });
        let worker_poll_interval_seconds = worker.poll_interval_seconds.unwrap_or(60);
        if worker.recovery_retry_delay_seconds == 0 || worker.recovery_retry_delay_seconds > 86_400
        {
            return Err(ConfigError::InvalidWorkerRecoveryRetryDelay);
        }
        validate_auction_worker(&worker, worker_concurrency, worker_poll_interval_seconds)?;
        validate_delivery_worker(&worker.delivery)?;
        validate_fulfillment_worker(&worker.fulfillment)?;
        validate_recovery_worker(&worker.recovery)?;
        validate_auction(&auction)?;
        validate_worker_control(&worker.control)?;
        validate_mooglemail_worker(&worker.mooglemail)?;
        validate_forum_worker(&worker.forum)?;
        validate_operations_alert_worker(&worker.operations_alert)?;
        validate_realtime(&realtime)?;
        let tracing_headers = match role {
            Role::Public => secrets.public_otlp_headers.as_ref(),
            Role::Admin => secrets.admin_otlp_headers.as_ref(),
            Role::Worker => secrets.worker_otlp_headers.as_ref(),
        };
        validate_tracing(&tracing, tracing_headers.map(SecretString::expose_secret))?;
        if role == Role::Worker
            && worker.mooglemail.enabled
            && worker.mooglemail.keyenv_file.is_none()
            && secrets.mooglemail_cookie.is_none()
            && secrets.credential_data_key_id.is_none()
        {
            return Err(ConfigError::MissingMoogleMailCredentials);
        }
        if role == Role::Worker
            && worker.mooglemail.enabled
            && (secrets.user_login_data_key_id.is_none() || secrets.user_login_data_key.is_none())
        {
            return Err(ConfigError::MissingMoogleMailUserLoginDataKey);
        }
        if role == Role::Worker
            && worker.forum.enabled
            && secrets.ehentai_cookie.is_none()
            && secrets.credential_data_key_id.is_none()
        {
            return Err(ConfigError::MissingEhentaiCredentials);
        }
        // 即使 Admin runtime 随后覆盖该区块，也拒绝主配置中无法解析的备用安全策略。
        validate_http_security(&http_security)?;
        validate_session(&session)?;
        if role == Role::Worker && database_is_sqlite && worker_concurrency != 1 {
            return Err(ConfigError::LiteWorkerConcurrency { concurrency: worker_concurrency });
        }

        let admin_uses_unix = role == Role::Admin
            && admin_runtime.as_ref().is_some_and(|runtime| {
                runtime.listener.mode == auction_application::AdminRuntimeListenMode::Unix
            });
        let runtime_listen_address = admin_runtime
            .as_ref()
            .filter(|runtime| {
                runtime.listener.mode == auction_application::AdminRuntimeListenMode::Tcp
            })
            .map(AdminRuntimeDocument::tcp_address)
            .transpose()
            .map_err(|error| ConfigError::InvalidRuntimeConfig {
                path: runtime_config_path.clone().unwrap_or_default(),
                reason: error.to_string(),
            })?
            .map(|address| address.to_string());
        let listen_value = if admin_uses_unix {
            None
        } else {
            runtime_listen_address.or_else(|| {
                listener
                    .as_ref()
                    .filter(|listener| listener.mode == HttpListenMode::Tcp)
                    .map(|listener| listener.address.clone())
            })
        };
        let http_security =
            admin_runtime.as_ref().map(runtime_http_security).unwrap_or(http_security);
        validate_http_security(&http_security)?;
        let public_uses_unix = role == Role::Public
            && listener.as_ref().is_some_and(|listener| listener.mode == HttpListenMode::Unix);
        if http_security.trusted_unix_proxy && !admin_uses_unix && !public_uses_unix {
            return Err(ConfigError::InvalidHttpSecurity(
                "trusted_unix_proxy 只能由 Public 或 Admin Unix listener 启用",
            ));
        }
        let session = admin_runtime.as_ref().map(runtime_session).unwrap_or(session);
        let listen_addr = listen_value
            .map(|value| {
                value.parse().map_err(|source| ConfigError::InvalidAddress {
                    field: "listen_address",
                    value,
                    source,
                })
            })
            .transpose()?;

        Ok(Self {
            role,
            listen_addr,
            listener,
            log_filter,
            metrics,
            tracing,
            realtime,
            cache,
            auction,
            worker,
            http_security,
            session,
            config_path,
            secrets_path,
            runtime_config_path,
            admin_runtime,
            secrets,
        })
    }
}
