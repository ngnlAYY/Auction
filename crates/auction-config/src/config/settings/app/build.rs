//! 敏感快照、runtime 覆盖和全部配置不变量的最终构建。

use super::super::*;
use super::model::{AppConfig, BuildInput};

impl AppConfig {
    pub(super) fn build(role: Role, input: BuildInput) -> Result<Self, ConfigError> {
        let BuildInput {
            config_path,
            common_config_path,
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

        let runtime_document = if matches!(role, Role::Admin | Role::Worker) {
            runtime_config_path.as_deref().map(load_admin_runtime_file).transpose()?
        } else {
            None
        };
        let admin_runtime =
            (role == Role::Admin).then(|| runtime_document.as_ref().cloned()).flatten();
        let mut auction = auction;
        let mut worker = worker;
        if let Some(runtime) = &runtime_document {
            apply_runtime_operations(&mut auction, &mut worker, role, runtime);
        }
        if role == Role::Worker
            && let Some(runtime) = &runtime_document
        {
            apply_runtime_document(&mut worker.mooglemail, runtime);
        }
        // 先保留主配置 listener，校验其 fallback 安全策略；runtime 覆盖必须在该阶段之后生效。
        let main_listener = resolve_http_listener(role, listener, listen_address)?;
        let main_listen_addr = main_listener
            .as_ref()
            .filter(|listener| listener.mode == HttpListenMode::Tcp)
            .map(|listener| {
                listener.address.parse().map_err(|source| ConfigError::InvalidAddress {
                    field: "listener.address",
                    value: listener.address.clone(),
                    source,
                })
            })
            .transpose()?;
        let mut http_security = http_security.unwrap_or_else(|| HttpSecurityConfig::for_role(role));
        let session = session.unwrap_or_else(|| SessionConfig::for_role(role));

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
        let worker_concurrency = worker.concurrency.unwrap_or(8);
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
        if worker.mooglemail.enabled
            && worker.delivery.request_timeout_seconds
                <= worker.mooglemail.fetch_timeout_seconds.saturating_mul(2)
        {
            return Err(ConfigError::InvalidDeliveryWorkerConfig(
                "MoogleMail 启用时 delivery.request_timeout_seconds 必须大于 fetch_timeout_seconds 的两倍",
            ));
        }
        validate_forum_worker(&worker.forum)?;
        validate_operations_alert_worker(&worker.operations_alert, &worker.mooglemail)?;
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
        resolve_http_security_origins(&mut http_security, main_listen_addr)?;
        validate_http_security(&http_security)?;
        validate_session(&session)?;

        let listener = if role == Role::Admin {
            runtime_document
                .as_ref()
                .map(admin_runtime_listener)
                .transpose()
                .map_err(|error| ConfigError::InvalidRuntimeConfig {
                    path: runtime_config_path.clone().unwrap_or_default(),
                    reason: error.to_string(),
                })?
                .or(main_listener)
        } else {
            main_listener
        };
        let effective_listen_addr = listener
            .as_ref()
            .filter(|listener| listener.mode == HttpListenMode::Tcp)
            .map(|listener| {
                listener.address.parse().map_err(|source| ConfigError::InvalidAddress {
                    field: "listener.address",
                    value: listener.address.clone(),
                    source,
                })
            })
            .transpose()?;
        let admin_uses_unix = role == Role::Admin
            && listener.as_ref().is_some_and(|listener| listener.mode == HttpListenMode::Unix);
        let listen_addr = if admin_uses_unix { None } else { effective_listen_addr };
        let mut http_security =
            admin_runtime.as_ref().map(runtime_http_security).unwrap_or(http_security);
        resolve_http_security_origins(&mut http_security, listen_addr)?;
        validate_http_security(&http_security)?;
        let public_uses_unix = role == Role::Public
            && listener.as_ref().is_some_and(|listener| listener.mode == HttpListenMode::Unix);
        if http_security.trusted_unix_proxy && !admin_uses_unix && !public_uses_unix {
            return Err(ConfigError::InvalidHttpSecurity(
                "trusted_unix_proxy 只能由 Public 或 Admin Unix listener 启用",
            ));
        }
        validate_metrics_listener(role, metrics, admin_uses_unix || public_uses_unix, listen_addr)?;
        let session = admin_runtime.as_ref().map(runtime_session).unwrap_or(session);

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
            common_config_path,
            secrets_path,
            runtime_config_path,
            admin_runtime,
            secrets,
        })
    }
}

fn admin_runtime_listener(
    runtime: &AdminRuntimeDocument,
) -> Result<HttpListenerConfig, ConfigError> {
    let listener = &runtime.listener;
    let effective = match listener.mode {
        auction_application::AdminRuntimeListenMode::Tcp => HttpListenerConfig {
            address: listener.address.clone(),
            group: String::new(),
            mode: HttpListenMode::Tcp,
            runtime_directory: String::new(),
            socket_path: String::new(),
        },
        auction_application::AdminRuntimeListenMode::Unix => HttpListenerConfig {
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
