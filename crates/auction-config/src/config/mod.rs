//! 进程 TOML 配置与敏感配置加载。
//!
//! 普通配置负责监听地址、缓存模式等非敏感设置；主配置可通过 `secrets_file` 引用独立的
//! 敏感 TOML。任何凭据都必须通过显式 getter 使用，不能依赖 `Debug` 输出或环境变量覆盖。

use std::{
    env, fmt, fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde::Deserialize;
use thiserror::Error;
use url::Url;

use auction_application::AdminRuntimeDocument;

const CONFIG_FILE_ENV: &str = "AUCTION_CONFIG_FILE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Public,
    Admin,
    Worker,
}

impl Role {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Admin => "admin",
            Self::Worker => "worker",
        }
    }

    const fn default_listen(self) -> Option<&'static str> {
        match self {
            Self::Public => Some("127.0.0.1:8090"),
            Self::Admin => Some("127.0.0.1:8091"),
            Self::Worker => None,
        }
    }
}

impl FromStr for Role {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "public" => Ok(Self::Public),
            "admin" => Ok(Self::Admin),
            "worker" => Ok(Self::Worker),
            _ => Err(ConfigError::UnknownRole(value.to_owned())),
        }
    }
}

/// 可由普通配置选择的缓存后端。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheBackend {
    /// 默认模式，不加载 Redis 客户端，也不建立缓存连接。
    #[default]
    Disabled,
    /// 使用敏感配置文件中的 `redis_url`。
    Redis,
}

impl CacheBackend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Redis => "redis",
        }
    }
}

/// 非敏感缓存策略。连接 URL 必须放在独立敏感配置文件中。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CacheConfig {
    /// 缓存后端；省略时关闭缓存。Redis 连接串只能放在敏感 TOML。
    pub backend: CacheBackend,
}

/// Worker 调度并发和数据库 due 轮询间隔。SQLite Lite 档只能使用单实例、单写者 Worker。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerConfig {
    /// Worker 并发数；省略时为 1。SQLite Lite 必须保持为 1。
    pub concurrency: u16,
    /// due 任务轮询间隔秒数；省略时为 5 秒。
    pub poll_interval_seconds: u64,
    /// 可选 MoogleMail 轮询配置；省略时关闭远程读取。
    pub mooglemail: MoogleMailWorkerConfig,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            concurrency: 1,
            poll_interval_seconds: 5,
            mooglemail: MoogleMailWorkerConfig::default(),
        }
    }
}

/// Worker 使用的 MoogleMail 入站轮询策略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MoogleMailWorkerConfig {
    /// 是否启用远程 MoogleMail 入站读取；默认关闭，避免配置不完整时发出外部请求。
    pub enabled: bool,
    /// 持久化邮箱标识；同一邮箱只能由一个有效租约并行处理。
    pub mailbox_key: String,
    /// 游戏世界，只允许 `main` 或 `isekai`。
    pub world_id: String,
    /// 可选 `.keyenv` 路径；相对路径相对于主 Worker 配置文件解析。
    pub keyenv_file: Option<PathBuf>,
    /// 单次从 MoogleMail 读取的最大邮件数。
    pub batch_size: u16,
    /// 单次有界分页最多读取的页数。
    pub maximum_pages: usize,
    /// 发件人 allowlist；未显式关闭 allowlist 时只读取其中的详情。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭发件人 allowlist 后才读取所有发件人详情。
    pub sender_allowlist_disabled: bool,
}

impl Default for MoogleMailWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mailbox_key: "mooglemail:main".to_owned(),
            world_id: "main".to_owned(),
            keyenv_file: None,
            batch_size: 100,
            maximum_pages: 20,
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: false,
        }
    }
}

/// 浏览器写请求和公开会话使用的非敏感 HTTP 安全策略。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpSecurityConfig {
    /// 允许的精确 HTTP(S) origin；启用 allowlist 时必须填写。
    pub allowed_origins: Vec<String>,
    /// 是否启用 origin allowlist；省略时关闭。
    pub allowed_origins_enabled: bool,
    /// 登录请求在一个窗口内的最大次数；省略时为 10 次。
    pub login_rate_limit: u32,
    /// 状态变更请求在一个窗口内的最大次数；省略时为 60 次。
    pub mutation_rate_limit: u32,
    /// 速率限制窗口秒数；省略时为 60 秒。
    pub rate_limit_window_seconds: u64,
    /// 是否为浏览器 Cookie 设置 Secure；省略时关闭，生产环境应开启。
    pub cookie_secure: bool,
}

impl Default for HttpSecurityConfig {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allowed_origins_enabled: false,
            login_rate_limit: 10,
            mutation_rate_limit: 60,
            rate_limit_window_seconds: 60,
            cookie_secure: false,
        }
    }
}

/// Public 会话的滑动过期、绝对过期和并发上限，单位为秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    /// 会话空闲过期秒数；省略时为 1800 秒。
    pub idle_ttl_seconds: u64,
    /// 会话绝对过期秒数；省略时为 604800 秒。
    pub absolute_ttl_seconds: u64,
    /// 单个公开用户允许的最大并发会话数；省略时为 5 个。
    pub max_public_sessions: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            idle_ttl_seconds: 30 * 60,
            absolute_ttl_seconds: 7 * 24 * 60 * 60,
            max_public_sessions: 5,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("unknown process role: {0}")]
    UnknownRole(String),

    #[error("failed to read TOML configuration {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    // 旧主配置可能仍含连接串或 Cookie，拒绝时也不能让解析器回显原始行。
    #[error("failed to parse TOML configuration {path}; configuration values are redacted")]
    Parse { path: PathBuf },

    #[error("failed to read sensitive TOML configuration {path}: {source}")]
    SecretRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    // 敏感 TOML 的解析器错误可能带出原始行，因此这里只报告文件路径。
    #[error("failed to parse sensitive TOML configuration {path}; secret values are redacted")]
    SecretParse { path: PathBuf },

    #[error("failed to read local MoogleMail key environment {path}: {source}")]
    MoogleMailKeyEnvRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("local MoogleMail key environment {path} must be owner-only readable")]
    MoogleMailKeyEnvInsecurePermissions { path: PathBuf },

    #[error("failed to parse local MoogleMail key environment {path}; secret values are redacted")]
    MoogleMailKeyEnvParse { path: PathBuf },

    #[error("local MoogleMail key environment {path} is missing {key}")]
    MoogleMailKeyEnvMissing { path: PathBuf, key: &'static str },

    #[error("invalid {field} value {value:?}: {source}")]
    InvalidAddress { field: &'static str, value: String, source: std::net::AddrParseError },

    #[error("sensitive field {field} must not be empty")]
    EmptySensitiveField { field: &'static str },

    #[error("cache backend redis requires sensitive field redis_url")]
    MissingRedisUrl,

    #[error("equipment data key requires both equipment_data_key_id and equipment_data_key")]
    InvalidEquipmentDataKeyConfiguration,

    #[error("worker concurrency must be greater than zero")]
    InvalidWorkerConcurrency,

    #[error("worker poll interval must be greater than zero")]
    InvalidWorkerPollInterval,

    #[error("enabled MoogleMail Worker requires keyenv_file or sensitive mooglemail_cookie")]
    MissingMoogleMailCredentials,

    #[error("invalid MoogleMail Worker configuration: {0}")]
    InvalidMoogleMailConfig(&'static str),

    #[error("SQLite Lite requires worker concurrency = 1, got {concurrency}")]
    LiteWorkerConcurrency { concurrency: u16 },

    #[error("invalid HTTP security configuration: {0}")]
    InvalidHttpSecurity(&'static str),

    #[error("invalid session configuration: {0}")]
    InvalidSessionConfig(&'static str),

    #[error("failed to read Admin runtime TOML configuration {path}: {source}")]
    RuntimeRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse Admin runtime TOML configuration {path}; values are redacted")]
    RuntimeParse { path: PathBuf },

    #[error("invalid Admin runtime configuration {path}: {reason}")]
    InvalidRuntimeConfig { path: PathBuf, reason: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    /// 可选监听地址；省略时使用角色默认地址，Worker 默认不监听。
    #[serde(default)]
    listen_address: Option<String>,
    /// 可选 tracing 过滤器；省略时由装配根采用默认过滤器。
    #[serde(default)]
    log_filter: Option<String>,
    /// 必填的敏感 TOML 相对或绝对路径。
    secrets_file: PathBuf,
    /// 可选的 Admin runtime 非敏感 TOML 路径；仅 Admin 会读取。
    #[serde(default)]
    runtime_config_file: Option<PathBuf>,
    /// 可选缓存区块；省略时使用关闭缓存的默认值。
    #[serde(default)]
    cache: CacheConfig,
    /// 可选 Worker 区块；省略时使用并发 1、轮询 5 秒的默认值。
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
    log_filter: Option<String>,
    cache: CacheConfig,
    worker: WorkerConfig,
    http_security: HttpSecurityConfig,
    session: SessionConfig,
    secrets_path: Option<PathBuf>,
    runtime_config_path: Option<PathBuf>,
}

/// 独立敏感 TOML 的稳定字段集合。新增外部凭据时必须显式扩展，禁止任意键值透传。
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SecretFileConfig {
    /// 数据库连接串；数据库进程按需读取，不能写入主配置。
    database_url: Option<String>,
    /// Redis 连接串；仅启用 Redis 缓存时需要。
    redis_url: Option<String>,
    /// 会话签名密钥；启用会话写入能力的角色需要。
    session_signing_key: Option<String>,
    /// E-Hentai Cookie；启用对应外部集成时需要。
    ehentai_cookie: Option<String>,
    /// MoogleMail Cookie；启用对应外部集成时需要。
    mooglemail_cookie: Option<String>,
    /// Admin 登录 actor；Admin 登录能力需要与 `admin_secret` 成对提供。
    admin_actor: Option<String>,
    /// Admin 登录 secret；Admin 登录能力需要与 `admin_actor` 成对提供。
    admin_secret: Option<String>,
    /// 装备数据密钥轮换标识；启用装备密钥封装时必须与材料成对提供。
    equipment_data_key_id: Option<String>,
    /// 装备数据密钥材料；只能放敏感文件，必须与标识成对提供。
    equipment_data_key: Option<String>,
}

/// 不会在 `Debug` 输出中暴露内容的敏感字符串。
#[derive(Clone, PartialEq, Eq)]
struct SecretString(Box<str>);

impl SecretString {
    fn new(field: &'static str, value: String) -> Result<Self, ConfigError> {
        if value.trim().is_empty() {
            return Err(ConfigError::EmptySensitiveField { field });
        }

        Ok(Self(value.into_boxed_str()))
    }

    fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// `.keyenv` 中仅允许出现的两个 MoogleMail Cookie 身份。
///
/// `hv-ck-key1` 固定解释为 Bot 号，`hv-ck-key2` 固定解释为测试号。该类型不实现序列化，
/// getter 只应在装配 MoogleMail 适配器时短暂调用。
#[derive(Clone)]
pub struct MoogleMailKeyEnv {
    bot_cookie: SecretString,
    test_cookie: SecretString,
}

impl MoogleMailKeyEnv {
    /// 返回 Bot 号 Cookie；调用方不得记录、持久化或传给浏览器。
    pub fn bot_cookie(&self) -> &str {
        self.bot_cookie.expose_secret()
    }

    /// 返回测试号 Cookie；调用方不得记录、持久化或传给浏览器。
    pub fn test_cookie(&self) -> &str {
        self.test_cookie.expose_secret()
    }
}

impl fmt::Debug for MoogleMailKeyEnv {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MoogleMailKeyEnv([REDACTED; 2])")
    }
}

#[derive(Clone, Default)]
struct AppSecrets {
    database_url: Option<SecretString>,
    redis_url: Option<SecretString>,
    session_signing_key: Option<SecretString>,
    ehentai_cookie: Option<SecretString>,
    mooglemail_cookie: Option<SecretString>,
    admin_actor: Option<SecretString>,
    admin_secret: Option<SecretString>,
    equipment_data_key_id: Option<SecretString>,
    equipment_data_key: Option<SecretString>,
}

impl AppSecrets {
    fn from_file(secret: SecretFileConfig) -> Result<Self, ConfigError> {
        Ok(Self {
            database_url: secret
                .database_url
                .map(|value| SecretString::new("database_url", value))
                .transpose()?,
            redis_url: secret
                .redis_url
                .map(|value| SecretString::new("redis_url", value))
                .transpose()?,
            session_signing_key: secret
                .session_signing_key
                .map(|value| SecretString::new("session_signing_key", value))
                .transpose()?,
            ehentai_cookie: secret
                .ehentai_cookie
                .map(|value| SecretString::new("ehentai_cookie", value))
                .transpose()?,
            mooglemail_cookie: secret
                .mooglemail_cookie
                .map(|value| SecretString::new("mooglemail_cookie", value))
                .transpose()?,
            admin_actor: secret
                .admin_actor
                .map(|value| SecretString::new("admin_actor", value))
                .transpose()?,
            admin_secret: secret
                .admin_secret
                .map(|value| SecretString::new("admin_secret", value))
                .transpose()?,
            equipment_data_key_id: secret
                .equipment_data_key_id
                .map(|value| SecretString::new("equipment_data_key_id", value))
                .transpose()?,
            equipment_data_key: secret
                .equipment_data_key
                .map(|value| SecretString::new("equipment_data_key", value))
                .transpose()?,
        })
    }
}

#[derive(Clone)]
pub struct AppConfig {
    /// 当前进程角色。
    pub role: Role,
    /// 解析后的可选监听地址；Worker 通常为 None。
    pub listen_addr: Option<SocketAddr>,
    /// 可选日志过滤器。
    pub log_filter: Option<String>,
    /// 非敏感缓存策略。
    pub cache: CacheConfig,
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
    secrets: AppSecrets,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("role", &self.role)
            .field("listen_addr", &self.listen_addr)
            .field("database_configured", &self.database_url().is_some())
            .field("admin_credentials_configured", &self.admin_credentials_configured())
            .field("cache", &self.cache)
            .field("worker", &self.worker)
            .field("http_security", &self.http_security)
            .field("session", &self.session)
            .field("log_filter", &self.log_filter)
            .field("config_path", &self.config_path)
            .field("secrets_path", &self.secrets_path)
            .field("runtime_config_path", &self.runtime_config_path)
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
                log_filter: None,
                cache: CacheConfig::default(),
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

    /// 判断 Admin 登录凭据是否成对配置，不暴露任何凭据内容。
    pub fn admin_credentials_configured(&self) -> bool {
        self.admin_actor().is_some() && self.admin_secret().is_some()
    }

    fn from_file(
        role: Role,
        config_path: Option<PathBuf>,
        file: FileConfig,
    ) -> Result<Self, ConfigError> {
        let FileConfig {
            listen_address,
            log_filter,
            secrets_file,
            runtime_config_file,
            cache,
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
                log_filter,
                cache,
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
            log_filter,
            cache,
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

        if secrets.equipment_data_key_id.is_some() != secrets.equipment_data_key.is_some() {
            return Err(ConfigError::InvalidEquipmentDataKeyConfiguration);
        }

        if cache.backend == CacheBackend::Redis && secrets.redis_url.is_none() {
            return Err(ConfigError::MissingRedisUrl);
        }
        if worker.concurrency == 0 {
            return Err(ConfigError::InvalidWorkerConcurrency);
        }
        if worker.poll_interval_seconds == 0 {
            return Err(ConfigError::InvalidWorkerPollInterval);
        }
        validate_mooglemail_worker(&worker.mooglemail)?;
        if role == Role::Worker
            && worker.mooglemail.enabled
            && worker.mooglemail.keyenv_file.is_none()
            && secrets.mooglemail_cookie.is_none()
        {
            return Err(ConfigError::MissingMoogleMailCredentials);
        }
        validate_http_security(&http_security)?;
        validate_session(&session)?;
        if role == Role::Worker
            && secrets.database_url.as_ref().is_some_and(|url| {
                url.expose_secret()
                    .split_once(':')
                    .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("sqlite"))
            })
            && worker.concurrency != 1
        {
            return Err(ConfigError::LiteWorkerConcurrency { concurrency: worker.concurrency });
        }

        let runtime_listen_address = admin_runtime
            .as_ref()
            .map(AdminRuntimeDocument::tcp_address)
            .transpose()
            .map_err(|error| ConfigError::InvalidRuntimeConfig {
                path: runtime_config_path.clone().unwrap_or_default(),
                reason: error.to_string(),
            })?
            .map(|address| address.to_string());
        let listen_value = runtime_listen_address
            .or(listen_address)
            .or_else(|| role.default_listen().map(str::to_owned));
        let http_security =
            admin_runtime.as_ref().map(runtime_http_security).unwrap_or(http_security);
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
            log_filter,
            cache,
            worker,
            http_security,
            session,
            config_path,
            secrets_path,
            runtime_config_path,
            secrets,
        })
    }
}

fn validate_http_security(config: &HttpSecurityConfig) -> Result<(), ConfigError> {
    if config.login_rate_limit == 0 || config.login_rate_limit > 10_000 {
        return Err(ConfigError::InvalidHttpSecurity("login_rate_limit 必须位于 1 至 10000"));
    }
    if config.mutation_rate_limit == 0 || config.mutation_rate_limit > 10_000 {
        return Err(ConfigError::InvalidHttpSecurity("mutation_rate_limit 必须位于 1 至 10000"));
    }
    if !(1..=3600).contains(&config.rate_limit_window_seconds) {
        return Err(ConfigError::InvalidHttpSecurity(
            "rate_limit_window_seconds 必须位于 1 至 3600",
        ));
    }
    if config.allowed_origins_enabled {
        if config.allowed_origins.is_empty() || config.allowed_origins.len() > 16 {
            return Err(ConfigError::InvalidHttpSecurity(
                "allowed_origins 启用时必须包含 1 至 16 个 origin",
            ));
        }
        for origin in &config.allowed_origins {
            let parsed = Url::parse(origin).map_err(|_| {
                ConfigError::InvalidHttpSecurity("allowed_origins 必须是有效的 HTTP(S) origin")
            })?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || parsed.username() != ""
                || parsed.password().is_some()
                || parsed.path() != ""
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.as_str() != origin
            {
                return Err(ConfigError::InvalidHttpSecurity(
                    "allowed_origins 只能包含精确 scheme+host",
                ));
            }
        }
    }
    Ok(())
}

fn validate_session(config: &SessionConfig) -> Result<(), ConfigError> {
    if !(60..=24 * 60 * 60).contains(&config.idle_ttl_seconds) {
        return Err(ConfigError::InvalidSessionConfig("idle_ttl_seconds 必须位于 60 至 86400"));
    }
    if config.absolute_ttl_seconds < config.idle_ttl_seconds
        || config.absolute_ttl_seconds > 30 * 24 * 60 * 60
    {
        return Err(ConfigError::InvalidSessionConfig(
            "absolute_ttl_seconds 必须不小于 idle_ttl_seconds 且不大于 2592000",
        ));
    }
    if config.max_public_sessions == 0 || config.max_public_sessions > 100 {
        return Err(ConfigError::InvalidSessionConfig("max_public_sessions 必须位于 1 至 100"));
    }
    Ok(())
}

fn resolve_secrets_path(config_path: Option<&Path>, requested_path: PathBuf) -> PathBuf {
    if requested_path.is_absolute() {
        return requested_path;
    }

    config_path.and_then(Path::parent).unwrap_or_else(|| Path::new(".")).join(requested_path)
}

fn resolve_config_path(config_path: Option<&Path>, requested_path: PathBuf) -> PathBuf {
    if requested_path.is_absolute() {
        return requested_path;
    }

    config_path.and_then(Path::parent).unwrap_or_else(|| Path::new(".")).join(requested_path)
}

fn resolve_worker_paths(config_path: Option<&Path>, mut worker: WorkerConfig) -> WorkerConfig {
    worker.mooglemail.keyenv_file =
        worker.mooglemail.keyenv_file.map(|path| resolve_config_path(config_path, path));
    worker
}

fn validate_mooglemail_worker(config: &MoogleMailWorkerConfig) -> Result<(), ConfigError> {
    if !valid_config_text(&config.mailbox_key, 256) {
        return Err(ConfigError::InvalidMoogleMailConfig("mailbox_key 无效"));
    }
    if !matches!(config.world_id.as_str(), "main" | "isekai") {
        return Err(ConfigError::InvalidMoogleMailConfig("world_id 只能是 main 或 isekai"));
    }
    if config.batch_size == 0 || config.batch_size > 1000 {
        return Err(ConfigError::InvalidMoogleMailConfig("batch_size 必须位于 1 至 1000"));
    }
    if config.maximum_pages == 0 || config.maximum_pages > 100 {
        return Err(ConfigError::InvalidMoogleMailConfig("maximum_pages 必须位于 1 至 100"));
    }
    for (index, sender) in config.sender_allowlist.iter().enumerate() {
        if !valid_config_text(sender, 256)
            || config.sender_allowlist[..index]
                .iter()
                .any(|previous| previous.eq_ignore_ascii_case(sender))
        {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "sender_allowlist 含有重复或无效发件人",
            ));
        }
    }
    Ok(())
}

fn valid_config_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn load_secret_file(path: &Path) -> Result<SecretFileConfig, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|source| ConfigError::SecretRead { path: path.to_owned(), source })?;

    toml::from_str(&contents).map_err(|_| ConfigError::SecretParse { path: path.to_owned() })
}

/// 读取本地 `.keyenv` 的两个 MoogleMail Cookie。
///
/// 这是用户明确提供的本地开发/联调凭据通道，不是通用环境变量覆盖机制。解析器只接受
/// `hv-ck-key1` 和 `hv-ck-key2`，拒绝未知键、重复键、空值和不安全文件权限；错误不会回显
/// 任何值。
pub fn load_mooglemail_keyenv(path: impl AsRef<Path>) -> Result<MoogleMailKeyEnv, ConfigError> {
    let path = path.as_ref().to_owned();
    let metadata = fs::metadata(&path)
        .map_err(|source| ConfigError::MoogleMailKeyEnvRead { path: path.clone(), source })?;
    if !metadata.is_file() {
        return Err(ConfigError::MoogleMailKeyEnvRead {
            path,
            source: io::Error::new(io::ErrorKind::InvalidInput, "path is not a regular file"),
        });
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(ConfigError::MoogleMailKeyEnvInsecurePermissions { path });
    }

    let contents = fs::read_to_string(&path)
        .map_err(|source| ConfigError::MoogleMailKeyEnvRead { path: path.clone(), source })?;
    let mut bot_cookie = None;
    let mut test_cookie = None;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            return Err(ConfigError::MoogleMailKeyEnvParse { path: path.clone() });
        };
        let key = raw_key.trim();
        let Some(value) = parse_mooglemail_keyenv_value(raw_value) else {
            return Err(ConfigError::MoogleMailKeyEnvParse { path: path.clone() });
        };
        match key {
            "hv-ck-key1" if bot_cookie.is_none() => bot_cookie = Some(value),
            "hv-ck-key2" if test_cookie.is_none() => test_cookie = Some(value),
            _ => return Err(ConfigError::MoogleMailKeyEnvParse { path: path.clone() }),
        }
    }
    let bot_cookie = bot_cookie.ok_or_else(|| ConfigError::MoogleMailKeyEnvMissing {
        path: path.clone(),
        key: "hv-ck-key1",
    })?;
    let test_cookie = test_cookie.ok_or_else(|| ConfigError::MoogleMailKeyEnvMissing {
        path: path.clone(),
        key: "hv-ck-key2",
    })?;
    Ok(MoogleMailKeyEnv {
        bot_cookie: SecretString::new("hv-ck-key1", bot_cookie)?,
        test_cookie: SecretString::new("hv-ck-key2", test_cookie)?,
    })
}

fn parse_mooglemail_keyenv_value(raw_value: &str) -> Option<String> {
    let value = if let Some(value) = raw_value.strip_prefix('"') {
        let value = value.strip_suffix('"')?;
        if value.contains(['"', '\\']) {
            return None;
        }
        value
    } else if let Some(value) = raw_value.strip_prefix('\'') {
        let value = value.strip_suffix('\'')?;
        if value.contains(['\'', '\\']) {
            return None;
        }
        value
    } else {
        if raw_value.contains(['"', '\'']) {
            return None;
        }
        raw_value
    };
    (!value.is_empty()
        && value.len() <= 4096
        && value == value.trim()
        && !value.chars().any(char::is_control))
    .then(|| value.to_owned())
}

fn load_admin_runtime_file(path: &Path) -> Result<AdminRuntimeDocument, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|source| ConfigError::RuntimeRead { path: path.to_owned(), source })?;
    let document: AdminRuntimeDocument = toml::from_str(&contents)
        .map_err(|_| ConfigError::RuntimeParse { path: path.to_owned() })?;
    document.validate().map_err(|error| ConfigError::InvalidRuntimeConfig {
        path: path.to_owned(),
        reason: error.to_string(),
    })?;
    Ok(document)
}

fn runtime_http_security(document: &AdminRuntimeDocument) -> HttpSecurityConfig {
    HttpSecurityConfig {
        allowed_origins: document.security.allowed_origins.clone(),
        allowed_origins_enabled: document.security.allowed_origins_enabled,
        login_rate_limit: document.security.login_rate_limit,
        mutation_rate_limit: document.security.mutation_rate_limit,
        rate_limit_window_seconds: document.security.rate_limit_window_seconds,
        cookie_secure: document.security.cookie_secure,
    }
}

fn runtime_session(document: &AdminRuntimeDocument) -> SessionConfig {
    SessionConfig {
        idle_ttl_seconds: document.session.idle_ttl_seconds,
        absolute_ttl_seconds: document.session.absolute_ttl_seconds,
        max_public_sessions: document.session.max_public_sessions,
    }
}
