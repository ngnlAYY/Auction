//! 敏感快照、runtime 覆盖和全部配置不变量的最终构建。

use tracing_subscriber::EnvFilter;

use auction_domain::DataKeyDomain;

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
            readiness,
            database_profiles,
            database_roles,
            database_pool,
            tracing,
            shutdown,
            realtime,
            backup,
            auction,
            worker,
            http_security,
            session,
            secrets_path,
            runtime_config_path,
            worker_config_path,
            runtime_worker_budget,
        } = input;
        let shutdown = shutdown.resolve(role)?;
        let secret_file =
            secrets_path.as_deref().map(load_secret_file).transpose()?.unwrap_or_default();
        let secrets = AppSecrets::from_file(secret_file)?;

        let mut runtime_document =
            runtime_config_path.as_deref().map(load_admin_runtime_file).transpose()?;
        let equipment_colors = runtime_document
            .as_ref()
            .and_then(|runtime| runtime.equipment_colors.as_ref())
            .map(|colors| colors.to_rules())
            .unwrap_or_default();
        // Public 注册提示和 Worker 履约只保留名单投影，不持有完整 Admin runtime 文档。
        let bot_players = if matches!(role, Role::Public | Role::Worker) {
            runtime_document
                .as_mut()
                .map(|runtime| std::mem::take(&mut runtime.bot_players))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut auction = auction;
        let mut worker = worker;
        // Public 仅展示装备分组和 bot 名单；创建者模板与 Worker 运行参数仍由原角色拥有。
        if matches!(role, Role::Admin | Role::Worker)
            && let Some(runtime) = &runtime_document
        {
            apply_runtime_operations(&mut auction, &mut worker, role, runtime);
        }
        if role == Role::Worker
            && let Some(runtime) = runtime_document.take()
        {
            apply_equipment_colors(&mut worker.forum, &runtime);
            apply_runtime_document(&mut worker.mooglemail, runtime);
        }
        // 投影完成后转移完整文档，避免 Admin 启动时复制全部模板、名单和规则。
        let admin_runtime = if role == Role::Admin { runtime_document } else { None };
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
        let session = session.unwrap_or_else(|| RoleSessionConfig::for_role(role));

        // Admin runtime 只覆盖指标启用状态；转发请求拒绝策略仍由部署配置持有，避免新增
        // runtime 字段改变文档指纹和命令摘要，也避免覆盖时把显式回退开关丢失。
        let metrics = admin_runtime
            .as_ref()
            .map(|runtime| MetricsConfig { enabled: runtime.metrics.enabled, ..metrics })
            .unwrap_or(metrics);
        let tracing = admin_runtime.as_ref().map(runtime_tracing).unwrap_or(tracing);

        if secrets.equipment_data_key_id.is_some() != secrets.equipment_data_key.is_some() {
            return Err(ConfigError::InvalidEquipmentDataKeyConfiguration);
        }
        if secrets.user_login_data_key_id.is_some() != secrets.user_login_data_key.is_some() {
            return Err(ConfigError::InvalidUserLoginDataKeyConfiguration);
        }
        if secrets.credential_data_key_id.is_some() != secrets.credential_data_key.is_some() {
            return Err(ConfigError::InvalidCredentialDataKeyConfiguration);
        }
        if secrets.mail_diagnostics_data_key_id.is_some()
            != secrets.mail_diagnostics_data_key.is_some()
        {
            return Err(ConfigError::InvalidDataKeyRing {
                domain: "mail_diagnostics",
                reason: "mail_diagnostics_data_key_id 与 mail_diagnostics_data_key 必须成对配置"
                    .to_owned(),
            });
        }
        // 旧密钥只能服务于已配置的当前密钥；缺失当前密钥的密钥环无法继续新写入。
        for (domain, current, previous) in [
            (
                DataKeyDomain::Equipment,
                secrets.equipment_data_key_id.is_some(),
                !secrets.equipment_previous_data_keys.is_empty(),
            ),
            (
                DataKeyDomain::UserLogin,
                secrets.user_login_data_key_id.is_some(),
                !secrets.user_login_previous_data_keys.is_empty(),
            ),
            (
                DataKeyDomain::Credential,
                secrets.credential_data_key_id.is_some(),
                !secrets.credential_previous_data_keys.is_empty(),
            ),
            (
                DataKeyDomain::MailDiagnostics,
                secrets.mail_diagnostics_data_key_id.is_some(),
                !secrets.mail_diagnostics_previous_data_keys.is_empty(),
            ),
        ] {
            if previous && !current {
                return Err(ConfigError::InvalidDataKeyRing {
                    domain: domain.as_str(),
                    reason: "配置旧密钥时必须同时配置当前密钥".to_owned(),
                });
            }
        }

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
        if !worker.mail_delivery_budget().is_valid() {
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
        validate_role_session(&session)?;

        let listener = if role == Role::Admin {
            admin_runtime
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
        // 没有 runtime 覆盖时沿用已校验的主配置，避免再次解析同一组 Origin 和 CIDR。
        if let Some(runtime) = &admin_runtime {
            http_security = runtime_http_security(runtime);
            resolve_http_security_origins(&mut http_security, listen_addr)?;
            validate_http_security(&http_security)?;
        }
        database_roles.validate()?;
        database_pool.validate(database_profiles, role, http_security.request_timeout_seconds)?;
        // 匿名读并发是进程内闸门：每次读最多占用该数量的服务池连接，出价等写请求不经过
        // 该信号量，因此读突发不能把出价排到读队列之后。省略时取 Public 服务池扣除
        // 1 个长期所有权预留后再折半，并至少留 1 个可用连接给写路径。
        if role == Role::Public {
            let available = database_pool.max_connections.saturating_sub(1);
            let limit = http_security
                .anonymous_read_concurrency
                .unwrap_or_else(|| (available.saturating_sub(1) / 2).max(1));
            if available > 1 && limit >= available {
                return Err(ConfigError::InvalidHttpSecurity(
                    "anonymous_read_concurrency 必须小于 Public 服务池扣除长期连接后的可用连接数",
                ));
            }
            http_security.anonymous_read_concurrency = Some(limit);
        }
        let public_uses_unix = role == Role::Public
            && listener.as_ref().is_some_and(|listener| listener.mode == HttpListenMode::Unix);
        if http_security.trusted_unix_proxy && !admin_uses_unix && !public_uses_unix {
            return Err(ConfigError::InvalidHttpSecurity(
                "trusted_unix_proxy 只能由 Public 或 Admin Unix listener 启用",
            ));
        }
        validate_metrics_listener(role, metrics, admin_uses_unix || public_uses_unix, listen_addr)?;
        let session = admin_runtime
            .as_ref()
            .map(|runtime| RoleSessionConfig::Timed(runtime_session(runtime)))
            .unwrap_or(session);

        // 与进程初始化共用过滤语法；放在既有校验之后，保留其他配置错误的优先级。
        if let Some(filter) = log_filter.as_deref() {
            EnvFilter::try_new(filter).map_err(|_| ConfigError::InvalidLogFilter)?;
        }

        Ok(Self {
            role,
            listen_addr,
            listener,
            log_filter,
            metrics,
            readiness,
            database_profiles,
            database_roles,
            database_pool,
            tracing,
            shutdown,
            realtime,
            backup,
            auction,
            worker,
            http_security,
            session,
            config_path,
            common_config_path,
            secrets_path,
            runtime_config_path,
            worker_config_path,
            runtime_worker_budget,
            admin_runtime,
            equipment_colors,
            bot_players,
            secrets,
        })
    }
}
