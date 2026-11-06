//! 敏感 TOML 模型、脱敏字符串和进程内部密钥容器。

use std::fmt;

use serde::Deserialize;

use auction_domain::DataKeyDomain;

use super::super::ConfigError;
use super::data_key::{PreviousDataKeyConfig, RetiredDataKey, retired_keys};
use super::database::{PostgresConnectionConfig, resolve_database_url};

/// 独立敏感 TOML 的稳定字段集合。新增外部凭据时必须显式扩展，禁止任意键值透传。
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SecretFileConfig {
    /// PostgreSQL 独立连接字段；不能与兼容的 `database_url` 同时提供。
    pub(crate) database: Option<PostgresConnectionConfig>,
    /// 兼容旧 PostgreSQL 连接串；数据库进程按需读取，不能写入主配置。
    pub(crate) database_url: Option<String>,
    /// 定时备份成功和失败的可选打卡地址；空字符串表示未配置。
    pub(crate) backup_heartbeat_success_url: Option<String>,
    pub(crate) backup_heartbeat_failure_url: Option<String>,
    /// 只限备份 bucket 的 R2 令牌；不得从环境或实例元数据解析。
    pub(crate) backup_r2_access_key_id: Option<String>,
    pub(crate) backup_r2_secret_access_key: Option<String>,
    /// 仅用于明确拒绝已废弃字段，不保留或使用其密钥材料。
    #[serde(rename = "session_signing_key")]
    pub(crate) obsolete_session_signing_key: Option<serde::de::IgnoredAny>,
    pub(crate) ehentai_cookie: Option<String>,
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
    /// MoogleMail 诊断归档的独立父密钥标识；与材料成对提供。
    pub(crate) mail_diagnostics_data_key_id: Option<String>,
    /// MoogleMail 诊断归档的独立父密钥材料；不与凭据/装备父密钥共用。
    pub(crate) mail_diagnostics_data_key: Option<String>,
    /// 装备域保留解密的旧密钥；只在轮换窗口使用。
    pub(crate) equipment_previous_data_keys: Vec<PreviousDataKeyConfig>,
    /// 用户登录域保留解密的旧密钥。
    pub(crate) user_login_previous_data_keys: Vec<PreviousDataKeyConfig>,
    /// 外部凭据域保留解密的旧密钥。
    pub(crate) credential_previous_data_keys: Vec<PreviousDataKeyConfig>,
    /// MoogleMail 诊断域保留解密的旧密钥。
    pub(crate) mail_diagnostics_previous_data_keys: Vec<PreviousDataKeyConfig>,
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

/// 维护 PostgreSQL 同集群 `postgres` 数据库所需的独立敏感模型。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PostgresMaintenanceSecretFile {
    /// 维护库的独立连接字段；与兼容 URL 二选一。
    pub(crate) database: Option<PostgresConnectionConfig>,
    /// 只供破坏性运维命令连接同集群 `postgres` 维护库的高权限 URL。
    pub(crate) database_url: Option<String>,
}

/// 独立 PostgreSQL 维护敏感 TOML 的脱敏结果。
/// 该类型只允许 `auction-cli` 的建库与重建命令在维护窗口短暂使用，不能并入 Public/Admin/Worker
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

/// 迁移 owner 专用的独立敏感 TOML：应用数据库 owner 账号的五字段连接参数。
/// 只有 `auction-cli` 的建库、迁移、重建与恢复可用；不得并入任何常驻角色的主配置或 runtime。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PostgresOwnerSecretFile {
    /// 应用数据库 owner 的独立连接字段；与兼容 URL 二选一。
    pub(crate) database: Option<PostgresConnectionConfig>,
    /// 兼容旧连接串写法；同样只指向应用数据库。
    pub(crate) database_url: Option<String>,
}

/// 独立 PostgreSQL owner 敏感 TOML 的脱敏结果。
/// 该类型只允许 `auction-cli` 在建库、迁移、重建与恢复窗口短暂使用，不能并入 Public/Admin/Worker
/// 常驻配置，也不会实现序列化或暴露在 `Debug` 输出中。
#[derive(Clone)]
pub struct PostgresOwnerSecrets {
    pub(crate) database_url: SecretString,
}

impl PostgresOwnerSecrets {
    /// 返回应用数据库 owner 的连接 URL；调用方不得记录或放入进程参数。
    pub fn database_url(&self) -> &str {
        self.database_url.expose_secret()
    }
}

impl fmt::Debug for PostgresOwnerSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PostgresOwnerSecrets([REDACTED])")
    }
}

/// `.keyenv` 中仅允许出现的两个 MoogleMail Cookie 身份。
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
    pub(crate) backup_heartbeat_success_url: Option<SecretString>,
    pub(crate) backup_heartbeat_failure_url: Option<SecretString>,
    pub(crate) backup_r2_access_key_id: Option<SecretString>,
    pub(crate) backup_r2_secret_access_key: Option<SecretString>,
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
    pub(crate) mail_diagnostics_data_key_id: Option<SecretString>,
    pub(crate) mail_diagnostics_data_key: Option<SecretString>,
    pub(crate) equipment_previous_data_keys: Vec<RetiredDataKey>,
    pub(crate) user_login_previous_data_keys: Vec<RetiredDataKey>,
    pub(crate) credential_previous_data_keys: Vec<RetiredDataKey>,
    pub(crate) mail_diagnostics_previous_data_keys: Vec<RetiredDataKey>,
}

impl AppSecrets {
    pub(crate) fn from_file(secret: SecretFileConfig) -> Result<Self, ConfigError> {
        if secret.obsolete_session_signing_key.is_some() {
            return Err(ConfigError::InvalidSessionConfig(
                "session_signing_key 从未用于会话认证，现已废弃；请从敏感 TOML 删除此字段，会话继续使用数据库中的不透明令牌",
            ));
        }
        if secret.admin_actor.is_some() != secret.admin_secret.is_some() {
            return Err(ConfigError::InvalidSessionConfig(
                "admin_actor 与 admin_secret 必须成对配置",
            ));
        }
        // 与正式会话共用校验，拒绝启动前无法表达或含控制字符的凭据；不回显原值。
        if let Some(actor) = &secret.admin_actor {
            auction_application::validate_admin_actor(actor)
                .map_err(|_| ConfigError::InvalidAdminActor)?;
        }
        if let Some(password) = &secret.admin_secret {
            auction_application::validate_admin_secret(password.as_bytes()).map_err(|_| {
                ConfigError::InvalidSessionConfig(
                    "admin_secret 必须为 16 至 4096 字节且不含 ASCII 控制字符",
                )
            })?;
        }
        // 旧密钥校验需要当前密钥标识；在字段被移入 `AppSecrets` 前先完成转换。
        let equipment_previous_data_keys = retired_keys(
            DataKeyDomain::Equipment,
            secret.equipment_data_key_id.as_deref(),
            secret.equipment_previous_data_keys,
        )?;
        let user_login_previous_data_keys = retired_keys(
            DataKeyDomain::UserLogin,
            secret.user_login_data_key_id.as_deref(),
            secret.user_login_previous_data_keys,
        )?;
        let credential_previous_data_keys = retired_keys(
            DataKeyDomain::Credential,
            secret.credential_data_key_id.as_deref(),
            secret.credential_previous_data_keys,
        )?;
        let mail_diagnostics_previous_data_keys = retired_keys(
            DataKeyDomain::MailDiagnostics,
            secret.mail_diagnostics_data_key_id.as_deref(),
            secret.mail_diagnostics_previous_data_keys,
        )?;
        Ok(Self {
            database_url: resolve_database_url(secret.database, secret.database_url)?,
            backup_heartbeat_success_url: secret
                .backup_heartbeat_success_url
                .filter(|value| !value.is_empty())
                .map(|value| SecretString::new("backup_heartbeat_success_url", value))
                .transpose()?,
            backup_heartbeat_failure_url: secret
                .backup_heartbeat_failure_url
                .filter(|value| !value.is_empty())
                .map(|value| SecretString::new("backup_heartbeat_failure_url", value))
                .transpose()?,
            backup_r2_access_key_id: secret
                .backup_r2_access_key_id
                .map(|value| SecretString::new("backup_r2_access_key_id", value))
                .transpose()?,
            backup_r2_secret_access_key: secret
                .backup_r2_secret_access_key
                .map(|value| SecretString::new("backup_r2_secret_access_key", value))
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
            mail_diagnostics_data_key_id: secret
                .mail_diagnostics_data_key_id
                .map(|value| SecretString::new("mail_diagnostics_data_key_id", value))
                .transpose()?,
            mail_diagnostics_data_key: secret
                .mail_diagnostics_data_key
                .map(|value| SecretString::new("mail_diagnostics_data_key", value))
                .transpose()?,
            equipment_previous_data_keys,
            user_login_previous_data_keys,
            credential_previous_data_keys,
            mail_diagnostics_previous_data_keys,
        })
    }
}
