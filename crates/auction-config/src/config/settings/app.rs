//! 应用配置文件模型、敏感快照装配和公共 getter。

use super::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    /// 兼容旧格式的可选监听地址；与 `listener` 同时提供会失败，建议使用完整 listener 区块。
    #[serde(default)]
    listen_address: Option<String>,
    /// 可选 HTTP listener 区块；Public/Admin 可选择 TCP 或 Linux Unix socket。
    #[serde(default)]
    listener: Option<HttpListenerConfig>,
    /// 可选 tracing 过滤器；省略时由装配根采用默认过滤器。
    #[serde(default)]
    log_filter: Option<String>,
    /// 可选 metrics 区块；省略时关闭进程本地 Prometheus 导出。
    #[serde(default)]
    metrics: MetricsConfig,
    /// 可选 tracing 区块；Public/Worker 使用，Admin 有 runtime 文件时由 runtime 覆盖。
    #[serde(default)]
    tracing: TracingSettings,
    /// 可选 Public realtime 区块；省略时使用稳定 consumer 和有界 relay 默认值。
    #[serde(default)]
    realtime: RealtimeConfig,
    /// 必填的敏感 TOML 相对或绝对路径。
    secrets_file: PathBuf,
    /// 可选的 Admin runtime 非敏感 TOML 路径；仅 Admin 会读取。
    #[serde(default)]
    runtime_config_file: Option<PathBuf>,
    /// 可选缓存区块；省略时使用关闭缓存的默认值。
    #[serde(default)]
    cache: CacheConfig,
    /// 可选拍卖策略区块；省略时使用旧版共享配置的默认模板。
    #[serde(default)]
    auction: AuctionConfig,
    /// 可选 Worker 区块；省略时 PostgreSQL 并发为 8、SQLite 并发为 1，轮询为 60 秒。
    #[serde(default)]
    worker: WorkerConfig,
    /// 可选 HTTP 安全区块；省略时使用内置默认值。
    #[serde(default)]
    http_security: HttpSecurityConfig,
    /// 可选会话区块；省略时使用默认过期和并发限制。
    #[serde(default)]
    session: SessionConfig,
}

struct BuildInput {
    config_path: Option<PathBuf>,
    listen_address: Option<String>,
    listener: Option<HttpListenerConfig>,
    log_filter: Option<String>,
    metrics: MetricsConfig,
    tracing: TracingSettings,
    realtime: RealtimeConfig,
    cache: CacheConfig,
    auction: AuctionConfig,
    worker: WorkerConfig,
    http_security: HttpSecurityConfig,
    session: SessionConfig,
    secrets_path: Option<PathBuf>,
    runtime_config_path: Option<PathBuf>,
}

#[derive(Clone)]
pub struct AppConfig {
    /// 当前进程角色。
    pub role: Role,
    /// 解析后的可选监听地址；Worker 通常为 None。
    pub listen_addr: Option<SocketAddr>,
    /// Public/Admin 主配置中的 listener；Worker 始终为 None。Admin runtime 存在时，实际
    /// Admin listener 由 runtime 文档覆盖，但该字段仍保留主配置的安全 fallback。
    pub listener: Option<HttpListenerConfig>,
    /// 可选日志过滤器。
    pub log_filter: Option<String>,
    /// 进程本地 Prometheus 指标导出策略。
    pub metrics: MetricsConfig,
    /// 当前角色的 OTLP/HTTP tracing 非敏感参数；Admin runtime 存在时已应用 runtime 覆盖。
    pub tracing: TracingSettings,
    /// Public realtime relay 与进程内连接 Hub 策略。
    pub realtime: RealtimeConfig,
    /// 非敏感缓存策略。
    pub cache: CacheConfig,
    /// 创建拍卖时冻结的经济、结算和恢复策略模板。
    pub auction: AuctionConfig,
    /// Worker 调度策略。
    pub worker: WorkerConfig,
    /// HTTP 安全策略。
    pub http_security: HttpSecurityConfig,
    /// 会话策略。
    pub session: SessionConfig,
    /// 主配置文件路径；程序化默认配置时为 None。
    pub config_path: Option<PathBuf>,
    /// 敏感配置文件路径；主配置加载时由必填 secrets_file 解析得到。
    pub secrets_path: Option<PathBuf>,
    /// Admin runtime 文件路径；非 Admin 或省略 runtime_config_file 时为 None。
    pub runtime_config_path: Option<PathBuf>,
    /// 已校验的 Admin runtime 非敏感文档；Worker/Public 始终为 None。
    pub admin_runtime: Option<AdminRuntimeDocument>,
    secrets: AppSecrets,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("role", &self.role)
            .field("listen_addr", &self.listen_addr)
            .field("listener", &self.listener)
            .field("database_configured", &self.database_url().is_some())
            .field("admin_credentials_configured", &self.admin_credentials_configured())
            .field("cache", &self.cache)
            .field("auction", &self.auction)
            .field("worker", &self.worker)
            .field("http_security", &self.http_security)
            .field("session", &self.session)
            .field("log_filter", &self.log_filter)
            .field("metrics", &self.metrics)
            .field("tracing", &self.tracing)
            .field("realtime", &self.realtime)
            .field("config_path", &self.config_path)
            .field("secrets_path", &self.secrets_path)
            .field("runtime_config_path", &self.runtime_config_path)
            .field("admin_runtime_configured", &self.admin_runtime.is_some())
            .finish()
    }
}

impl AppConfig {
    pub fn load(role: Role) -> Result<Self, ConfigError> {
        if let Some(path) = env::var_os(CONFIG_FILE_ENV).map(PathBuf::from) {
            return Self::load_from_path(role, path);
        }

        let default_path = PathBuf::from("ops/config").join(format!("{}.toml", role.name()));
        if default_path.is_file() {
            return Self::load_from_path(role, default_path);
        }

        Self::build(
            role,
            BuildInput {
                config_path: None,
                listen_address: None,
                listener: None,
                log_filter: None,
                metrics: MetricsConfig::default(),
                tracing: TracingSettings::default(),
                realtime: RealtimeConfig::default(),
                cache: CacheConfig::default(),
                auction: AuctionConfig::default(),
                worker: WorkerConfig::default(),
                http_security: HttpSecurityConfig::default(),
                session: SessionConfig::default(),
                secrets_path: None,
                runtime_config_path: None,
            },
        )
    }

    pub fn load_from_path(role: Role, path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref().to_owned();
        let contents = fs::read_to_string(&path)
            .map_err(|source| ConfigError::Read { path: path.clone(), source })?;
        let file =
            toml::from_str(&contents).map_err(|_| ConfigError::Parse { path: path.clone() })?;

        Self::from_file(role, Some(path), file)
    }

    /// 返回数据库 URL。调用方不得把返回值写入日志、错误消息或 DTO。
    pub fn database_url(&self) -> Option<&str> {
        self.secrets.database_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回当前数据库档位下的有效 Worker 并发；SQLite Lite 的省略值固定为 1。
    pub fn worker_concurrency(&self) -> u16 {
        self.worker.concurrency.unwrap_or_else(|| {
            if self.database_url().is_some_and(database_url_is_sqlite) { 1 } else { 8 }
        })
    }

    /// 返回有效的 Worker 轮询间隔秒数；省略时沿用旧版 60 秒。
    pub fn worker_poll_interval_seconds(&self) -> u64 {
        self.worker.poll_interval_seconds.unwrap_or(60)
    }

    /// 返回 Redis URL。只有 `cache.backend = "redis"` 时才能使用。
    pub fn redis_url(&self) -> Option<&str> {
        self.secrets.redis_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回会话签名密钥。调用方不得输出、持久化副本或传给浏览器。
    pub fn session_signing_key(&self) -> Option<&str> {
        self.secrets.session_signing_key.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 E-Hentai Cookie。只允许外部集成适配器读取。
    pub fn ehentai_cookie(&self) -> Option<&str> {
        self.secrets.ehentai_cookie.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 MoogleMail Cookie。只允许外部集成适配器读取。
    pub fn mooglemail_cookie(&self) -> Option<&str> {
        self.secrets.mooglemail_cookie.as_ref().map(SecretString::expose_secret)
    }

    /// 返回当前角色的 OTLP headers；调用方只允许把它交给 tracing exporter，禁止记录或持久化。
    pub fn tracing_headers(&self) -> Option<&str> {
        match self.role {
            Role::Public => self.secrets.public_otlp_headers.as_ref(),
            Role::Admin => self.secrets.admin_otlp_headers.as_ref(),
            Role::Worker => self.secrets.worker_otlp_headers.as_ref(),
        }
        .map(SecretString::expose_secret)
    }

    /// 返回 Admin 登录 actor；只允许 Admin 装配根读取。
    pub fn admin_actor(&self) -> Option<&str> {
        self.secrets.admin_actor.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 Admin 登录 secret；调用方不得输出、持久化副本或传给浏览器。
    pub fn admin_secret(&self) -> Option<&str> {
        self.secrets.admin_secret.as_ref().map(SecretString::expose_secret)
    }

    /// 返回装备 key 加密使用的数据密钥标识；只允许密钥装配根读取。
    pub fn equipment_data_key_id(&self) -> Option<&str> {
        self.secrets.equipment_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回装备 key 加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn equipment_data_key(&self) -> Option<&str> {
        self.secrets.equipment_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 判断装备 key 加密配置是否成对提供。
    pub fn equipment_data_key_configured(&self) -> bool {
        self.equipment_data_key_id().is_some() && self.equipment_data_key().is_some()
    }

    /// 返回用户登录 key 加密使用的数据密钥标识；只允许密钥装配根读取。
    pub fn user_login_data_key_id(&self) -> Option<&str> {
        self.secrets.user_login_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回用户登录 key 加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn user_login_data_key(&self) -> Option<&str> {
        self.secrets.user_login_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 判断用户登录 key 加密配置是否成对提供。
    pub fn user_login_data_key_configured(&self) -> bool {
        self.user_login_data_key_id().is_some() && self.user_login_data_key().is_some()
    }

    /// 返回外部凭据加密使用的数据密钥标识；只允许凭据装配根读取。
    pub fn credential_data_key_id(&self) -> Option<&str> {
        self.secrets.credential_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回外部凭据加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn credential_data_key(&self) -> Option<&str> {
        self.secrets.credential_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 判断外部凭据加密配置是否成对提供。
    pub fn credential_data_key_configured(&self) -> bool {
        self.credential_data_key_id().is_some() && self.credential_data_key().is_some()
    }

    /// 判断 Admin 登录凭据是否成对配置，不暴露任何凭据内容。
    pub fn admin_credentials_configured(&self) -> bool {
        self.admin_actor().is_some() && self.admin_secret().is_some()
    }

    /// 返回 Admin dashboard 连接 Worker control 使用的 socket 路径。
    ///
    /// runtime 文档省略时使用应用模型的默认路径；该 getter 不读取环境变量，也不暴露敏感值。
    pub fn worker_control_socket_path(&self) -> &str {
        self.admin_runtime
            .as_ref()
            .map(|document| document.worker_control.socket_path.as_str())
            .unwrap_or("/run/hv-auction/worker/control.sock")
    }

    /// 返回 Admin runtime 选择的 Unix listener socket；TCP 模式和其它角色返回 `None`。
    ///
    /// 该 getter 只暴露非敏感监听路径，供部署预检和 readiness 探针与 Admin 装配根使用同一
    /// 权威配置，避免部署控制器仍探测已经被 runtime 覆盖掉的 TCP 地址。
    pub fn admin_unix_listener_socket_path(&self) -> Option<&str> {
        self.admin_runtime.as_ref().and_then(|document| {
            (document.listener.mode == auction_application::AdminRuntimeListenMode::Unix)
                .then_some(document.listener.socket_path.as_str())
        })
    }

    fn from_file(
        role: Role,
        config_path: Option<PathBuf>,
        file: FileConfig,
    ) -> Result<Self, ConfigError> {
        let FileConfig {
            listen_address,
            listener,
            log_filter,
            metrics,
            tracing,
            realtime,
            secrets_file,
            runtime_config_file,
            cache,
            auction,
            worker,
            http_security,
            session,
        } = file;
        let worker = resolve_worker_paths(config_path.as_deref(), worker);
        let secrets_path = Some(resolve_secrets_path(config_path.as_deref(), secrets_file));
        let runtime_config_path =
            runtime_config_file.map(|path| resolve_config_path(config_path.as_deref(), path));

        Self::build(
            role,
            BuildInput {
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
            },
        )
    }

    fn build(role: Role, input: BuildInput) -> Result<Self, ConfigError> {
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
