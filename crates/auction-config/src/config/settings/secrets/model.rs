//! 敏感 TOML 模型、脱敏字符串和进程内部密钥容器。

use std::fmt;

use serde::Deserialize;

use super::super::ConfigError;

/// 独立敏感 TOML 的稳定字段集合。新增外部凭据时必须显式扩展，禁止任意键值透传。
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SecretFileConfig {
    /// 数据库连接串；数据库进程按需读取，不能写入主配置。
    pub(crate) database_url: Option<String>,
    /// Redis 连接串；仅启用 Redis 缓存时需要。
    pub(crate) redis_url: Option<String>,
    /// 会话签名密钥；启用会话写入能力的角色需要。
    pub(crate) session_signing_key: Option<String>,
    /// E-Hentai Cookie；启用对应外部集成时需要。
    pub(crate) ehentai_cookie: Option<String>,
    /// MoogleMail Cookie；启用对应外部集成时需要。
    pub(crate) mooglemail_cookie: Option<String>,
    /// Public OTLP headers；只允许放在敏感 TOML，关闭 tracing 时不得提供。
    pub(crate) public_otlp_headers: Option<String>,
    /// Worker OTLP headers；只允许放在敏感 TOML，关闭 tracing 时不得提供。
    pub(crate) worker_otlp_headers: Option<String>,
    /// Admin OTLP headers；只允许放在敏感 TOML，关闭 tracing 时不得提供。
    pub(crate) admin_otlp_headers: Option<String>,
    /// Admin 登录 actor；Admin 登录能力需要与 `admin_secret` 成对提供。
    pub(crate) admin_actor: Option<String>,
    /// Admin 登录 secret；Admin 登录能力需要与 `admin_actor` 成对提供。
    pub(crate) admin_secret: Option<String>,
    /// 装备数据密钥轮换标识；启用装备密钥封装时必须与材料成对提供。
    pub(crate) equipment_data_key_id: Option<String>,
    /// 装备数据密钥材料；只能放敏感文件，必须与标识成对提供。
    pub(crate) equipment_data_key: Option<String>,
    /// 用户登录 key 数据密钥轮换标识；注册/换绑能力需要与材料成对提供。
    pub(crate) user_login_data_key_id: Option<String>,
    /// 用户登录 key 数据密钥材料；只能放敏感文件，不能与装备 key 混用。
    pub(crate) user_login_data_key: Option<String>,
    /// 外部凭据数据密钥轮换标识；A32 凭据密封需要与材料成对提供。
    pub(crate) credential_data_key_id: Option<String>,
    /// 外部凭据数据密钥材料；只能放敏感文件，不能与其他用途数据密钥混用。
    pub(crate) credential_data_key: Option<String>,
}

/// 不会在 `Debug` 输出中暴露内容的敏感字符串。
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SecretString(pub(crate) Box<str>);

impl SecretString {
    pub(crate) fn new(field: &'static str, value: String) -> Result<Self, ConfigError> {
        if value.trim().is_empty() {
            return Err(ConfigError::EmptySensitiveField { field });
        }

        Ok(Self(value.into_boxed_str()))
    }

    pub(crate) fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 维护 PostgreSQL 同集群 `postgres` 数据库所需的单字段敏感模型。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PostgresMaintenanceSecretFile {
    /// 只供破坏性运维命令连接同集群 `postgres` 维护库的高权限 URL。
    pub(crate) database_url: String,
}

/// 独立 PostgreSQL 维护敏感 TOML 的脱敏结果。
///
/// 该类型只允许 `auctionctl postgres-rebuild` 在维护窗口短暂使用，不能并入 Public/Admin/Worker
/// 常驻配置，也不会实现序列化或暴露在 `Debug` 输出中。
#[derive(Clone)]
pub struct PostgresMaintenanceSecrets {
    pub(crate) database_url: SecretString,
}

impl PostgresMaintenanceSecrets {
    /// 返回指向同集群 `postgres` 数据库的维护 URL；调用方不得记录或放入进程参数。
    pub fn database_url(&self) -> &str {
        self.database_url.expose_secret()
    }
}

impl fmt::Debug for PostgresMaintenanceSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PostgresMaintenanceSecrets([REDACTED])")
    }
}

/// `.keyenv` 中仅允许出现的两个 MoogleMail Cookie 身份。
///
/// `hv-ck-key1` 固定解释为 Worker 运行账号，`hv-ck-key2` 只供显式隔离验证。该类型不实现
/// 序列化；生产装配不得把二者组成候选池，也不得自动切换到测试号。
#[derive(Clone)]
pub struct MoogleMailKeyEnv {
    pub(crate) bot_cookie: SecretString,
    pub(crate) test_cookie: SecretString,
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

/// 已通过非敏感字段校验的进程内部敏感配置。
#[derive(Clone, Default)]
pub(crate) struct AppSecrets {
    pub(crate) database_url: Option<SecretString>,
    pub(crate) redis_url: Option<SecretString>,
    pub(crate) session_signing_key: Option<SecretString>,
    pub(crate) ehentai_cookie: Option<SecretString>,
    pub(crate) mooglemail_cookie: Option<SecretString>,
    pub(crate) public_otlp_headers: Option<SecretString>,
    pub(crate) worker_otlp_headers: Option<SecretString>,
    pub(crate) admin_otlp_headers: Option<SecretString>,
    pub(crate) admin_actor: Option<SecretString>,
    pub(crate) admin_secret: Option<SecretString>,
    pub(crate) equipment_data_key_id: Option<SecretString>,
    pub(crate) equipment_data_key: Option<SecretString>,
    pub(crate) user_login_data_key_id: Option<SecretString>,
    pub(crate) user_login_data_key: Option<SecretString>,
    pub(crate) credential_data_key_id: Option<SecretString>,
    pub(crate) credential_data_key: Option<SecretString>,
}

impl AppSecrets {
    pub(crate) fn from_file(secret: SecretFileConfig) -> Result<Self, ConfigError> {
        // 提前拒绝 Basic Auth 不能表达的账号，避免配置检查通过后启动一个无法登录的管理端。
        if secret.admin_actor.as_ref().is_some_and(|actor| actor.contains(':')) {
            return Err(ConfigError::InvalidAdminActor);
        }
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
            public_otlp_headers: secret
                .public_otlp_headers
                .map(|value| SecretString::new("public_otlp_headers", value))
                .transpose()?,
            worker_otlp_headers: secret
                .worker_otlp_headers
                .map(|value| SecretString::new("worker_otlp_headers", value))
                .transpose()?,
            admin_otlp_headers: secret
                .admin_otlp_headers
                .map(|value| SecretString::new("admin_otlp_headers", value))
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
            user_login_data_key_id: secret
                .user_login_data_key_id
                .map(|value| SecretString::new("user_login_data_key_id", value))
                .transpose()?,
            user_login_data_key: secret
                .user_login_data_key
                .map(|value| SecretString::new("user_login_data_key", value))
                .transpose()?,
            credential_data_key_id: secret
                .credential_data_key_id
                .map(|value| SecretString::new("credential_data_key_id", value))
                .transpose()?,
            credential_data_key: secret
                .credential_data_key
                .map(|value| SecretString::new("credential_data_key", value))
                .transpose()?,
        })
    }
}
