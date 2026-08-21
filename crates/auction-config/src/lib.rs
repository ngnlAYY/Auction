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

use serde::Deserialize;
use thiserror::Error;

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
    pub backend: CacheBackend,
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

    #[error("invalid {field} value {value:?}: {source}")]
    InvalidAddress { field: &'static str, value: String, source: std::net::AddrParseError },

    #[error("sensitive field {field} must not be empty")]
    EmptySensitiveField { field: &'static str },

    #[error("cache backend redis requires sensitive field redis_url")]
    MissingRedisUrl,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    listen_address: Option<String>,
    log_filter: Option<String>,
    secrets_file: PathBuf,
    #[serde(default)]
    cache: CacheConfig,
}

/// 独立敏感 TOML 的稳定字段集合。新增外部凭据时必须显式扩展，禁止任意键值透传。
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretFileConfig {
    database_url: Option<String>,
    redis_url: Option<String>,
    session_signing_key: Option<String>,
    ehentai_cookie: Option<String>,
    mooglemail_cookie: Option<String>,
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

#[derive(Clone, Default)]
struct AppSecrets {
    database_url: Option<SecretString>,
    redis_url: Option<SecretString>,
    session_signing_key: Option<SecretString>,
    ehentai_cookie: Option<SecretString>,
    mooglemail_cookie: Option<SecretString>,
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
        })
    }
}

#[derive(Clone)]
pub struct AppConfig {
    pub role: Role,
    pub listen_addr: Option<SocketAddr>,
    pub log_filter: Option<String>,
    pub cache: CacheConfig,
    pub config_path: Option<PathBuf>,
    pub secrets_path: Option<PathBuf>,
    secrets: AppSecrets,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("role", &self.role)
            .field("listen_addr", &self.listen_addr)
            .field("database_configured", &self.database_url().is_some())
            .field("cache", &self.cache)
            .field("log_filter", &self.log_filter)
            .field("config_path", &self.config_path)
            .field("secrets_path", &self.secrets_path)
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

        Self::build(role, None, None, None, CacheConfig::default(), None)
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

    fn from_file(
        role: Role,
        config_path: Option<PathBuf>,
        file: FileConfig,
    ) -> Result<Self, ConfigError> {
        let FileConfig { listen_address, log_filter, secrets_file, cache } = file;
        let secrets_path = Some(resolve_secrets_path(config_path.as_deref(), secrets_file));

        Self::build(role, config_path, listen_address, log_filter, cache, secrets_path)
    }

    fn build(
        role: Role,
        config_path: Option<PathBuf>,
        listen_address: Option<String>,
        log_filter: Option<String>,
        cache: CacheConfig,
        secrets_path: Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let secret_file =
            secrets_path.as_deref().map(load_secret_file).transpose()?.unwrap_or_default();
        let secrets = AppSecrets::from_file(secret_file)?;

        if cache.backend == CacheBackend::Redis && secrets.redis_url.is_none() {
            return Err(ConfigError::MissingRedisUrl);
        }

        let listen_value = listen_address.or_else(|| role.default_listen().map(str::to_owned));
        let listen_addr = listen_value
            .map(|value| {
                value.parse().map_err(|source| ConfigError::InvalidAddress {
                    field: "listen_address",
                    value,
                    source,
                })
            })
            .transpose()?;

        Ok(Self { role, listen_addr, log_filter, cache, config_path, secrets_path, secrets })
    }
}

fn resolve_secrets_path(config_path: Option<&Path>, requested_path: PathBuf) -> PathBuf {
    if requested_path.is_absolute() {
        return requested_path;
    }

    config_path.and_then(Path::parent).unwrap_or_else(|| Path::new(".")).join(requested_path)
}

fn load_secret_file(path: &Path) -> Result<SecretFileConfig, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|source| ConfigError::SecretRead { path: path.to_owned(), source })?;

    toml::from_str(&contents).map_err(|_| ConfigError::SecretParse { path: path.to_owned() })
}
