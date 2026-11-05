//! 配置解析、敏感字段和运行时文档错误。

use super::*;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("数据库连接分档配置无效：{0}")]
    InvalidDatabaseProfile(&'static str),
    #[error("数据库账号分工配置无效：{0}")]
    InvalidDatabaseRole(&'static str),
    #[error("shutdown 普通预算须为 1-300 秒，关键段须为 0-300 秒且仅 Worker 可非零")]
    InvalidShutdownConfig,
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

    #[error("PostgreSQL maintenance sensitive TOML path is invalid: {path}")]
    InvalidMaintenanceSecretPath { path: PathBuf },

    #[error("PostgreSQL maintenance sensitive TOML must be a regular non-symlink file: {path}")]
    InvalidMaintenanceSecretFile { path: PathBuf },

    #[error("PostgreSQL maintenance sensitive TOML must be owner-only readable: {path}")]
    InsecureMaintenanceSecretPermissions { path: PathBuf },

    #[error(
        "PostgreSQL maintenance sensitive TOML parent must not allow group/other writes: {path}"
    )]
    InsecureMaintenanceSecretParent { path: PathBuf },

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

    #[error("数据库配置字段 {0} 无效；主机、端口、账号、密码、数据库名称必须分别填写")]
    InvalidDatabaseConfig(&'static str),

    #[error("数据库配置不能同时提供 [database] 和 database_url，请保留一种写法")]
    ConflictingDatabaseConfiguration,

    #[error("admin_actor 不能包含 Basic Auth 分隔符冒号")]
    InvalidAdminActor,

    #[error("cache backend redis requires sensitive field redis_url")]
    MissingRedisUrl,

    #[error("invalid cache configuration: {0}")]
    InvalidCacheConfig(&'static str),

    #[error("定时备份配置无效：{0}")]
    InvalidBackupConfig(&'static str),

    #[error("异机备份字段 {field} 无效：{reason}；配置值已脱敏")]
    InvalidBackupOffsite { field: &'static str, reason: &'static str },

    #[error("敏感字段 {field} 必须是无 userinfo 和 fragment 的 HTTPS URL；配置值已脱敏")]
    InvalidBackupHeartbeatUrl { field: &'static str },

    #[error("invalid HTTP listener configuration: {0}")]
    InvalidListenerConfig(&'static str),

    #[error("equipment data key requires both equipment_data_key_id and equipment_data_key")]
    InvalidEquipmentDataKeyConfiguration,

    #[error("user login data key requires both user_login_data_key_id and user_login_data_key")]
    InvalidUserLoginDataKeyConfiguration,

    #[error("credential data key requires both credential_data_key_id and credential_data_key")]
    InvalidCredentialDataKeyConfiguration,

    #[error("worker recovery retry delay must be between 1 and 86400 seconds")]
    InvalidWorkerRecoveryRetryDelay,

    #[error("invalid auction Worker configuration: {0}")]
    InvalidAuctionWorkerConfig(&'static str),

    #[error("invalid delivery Worker configuration: {0}")]
    InvalidDeliveryWorkerConfig(&'static str),

    #[error("invalid fulfillment Worker configuration: {0}")]
    InvalidFulfillmentWorkerConfig(&'static str),

    #[error("invalid recovery Worker configuration: {0}")]
    InvalidRecoveryWorkerConfig(&'static str),

    #[error("invalid Worker control configuration: {0}")]
    InvalidWorkerControlConfig(&'static str),

    #[error(
        "enabled MoogleMail Worker requires keyenv_file, sensitive mooglemail_cookie, or credential data key"
    )]
    MissingMoogleMailCredentials,

    #[error("enabled MoogleMail Worker requires user_login_data_key_id and user_login_data_key")]
    MissingMoogleMailUserLoginDataKey,

    #[error("invalid MoogleMail Worker configuration: {0}")]
    InvalidMoogleMailConfig(&'static str),

    #[error("invalid auction policy configuration: {0}")]
    InvalidAuctionConfig(&'static str),

    #[error(
        "enabled E-Hentai forum Worker requires sensitive ehentai_cookie or credential data key"
    )]
    MissingEhentaiCredentials,

    #[error("invalid E-Hentai forum Worker configuration: {0}")]
    InvalidForumConfig(&'static str),

    #[error("invalid operations alert Worker configuration: {0}")]
    InvalidOperationsAlertConfig(&'static str),

    #[error("invalid realtime configuration: {0}")]
    InvalidRealtimeConfig(&'static str),

    #[error("log_filter 表达式无效；配置值已脱敏")]
    InvalidLogFilter,

    #[error("invalid tracing configuration: {0}")]
    InvalidTracingConfig(&'static str),

    #[error("invalid metrics configuration: {0}")]
    InvalidMetricsConfig(&'static str),

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

    #[error("Admin runtime Worker 部署引用无效：{0}")]
    InvalidRuntimeWorkerConfig(&'static str),

    #[error(
        "{role} 角色文件不支持 [{section}] 区块：该角色从不读取此区块，保留它会让配置看起来生效。请删除该区块，或将对应设置写入拥有它的配置文件；区块与 owner 的对照见 ops/config/README.md"
    )]
    UnsupportedRoleSection { role: &'static str, section: &'static str },

    #[error(
        "Admin 角色文件的 [session] max_public_sessions 不会生效：Admin 进程从不限制公开用户会话数。请删除该字段；公开会话上限只属于 public.toml 的 [session] 与共享 runtime 文档，见 ops/config/README.md"
    )]
    AdminSessionLimitNotApplied,
}
