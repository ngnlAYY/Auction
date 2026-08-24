//! 敏感配置模型、脱敏 getter 和安全文件读取。

use super::*;

/// 独立敏感 TOML 的稳定字段集合。新增外部凭据时必须显式扩展，禁止任意键值透传。
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct SecretFileConfig {
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
    /// Public OTLP headers；只允许放在敏感 TOML，关闭 tracing 时不得提供。
    public_otlp_headers: Option<String>,
    /// Worker OTLP headers；只允许放在敏感 TOML，关闭 tracing 时不得提供。
    worker_otlp_headers: Option<String>,
    /// Admin OTLP headers；只允许放在敏感 TOML，关闭 tracing 时不得提供。
    admin_otlp_headers: Option<String>,
    /// Admin 登录 actor；Admin 登录能力需要与 `admin_secret` 成对提供。
    admin_actor: Option<String>,
    /// Admin 登录 secret；Admin 登录能力需要与 `admin_actor` 成对提供。
    admin_secret: Option<String>,
    /// 装备数据密钥轮换标识；启用装备密钥封装时必须与材料成对提供。
    equipment_data_key_id: Option<String>,
    /// 装备数据密钥材料；只能放敏感文件，必须与标识成对提供。
    equipment_data_key: Option<String>,
    /// 用户登录 key 数据密钥轮换标识；注册/换绑能力需要与材料成对提供。
    user_login_data_key_id: Option<String>,
    /// 用户登录 key 数据密钥材料；只能放敏感文件，不能与装备 key 混用。
    user_login_data_key: Option<String>,
    /// 外部凭据数据密钥轮换标识；A32 凭据密封需要与材料成对提供。
    credential_data_key_id: Option<String>,
    /// 外部凭据数据密钥材料；只能放敏感文件，不能与其他用途数据密钥混用。
    credential_data_key: Option<String>,
}

/// 不会在 `Debug` 输出中暴露内容的敏感字符串。
#[derive(Clone, PartialEq, Eq)]
pub(super) struct SecretString(Box<str>);

impl SecretString {
    fn new(field: &'static str, value: String) -> Result<Self, ConfigError> {
        if value.trim().is_empty() {
            return Err(ConfigError::EmptySensitiveField { field });
        }

        Ok(Self(value.into_boxed_str()))
    }

    pub(super) fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostgresMaintenanceSecretFile {
    /// 只供破坏性运维命令连接同集群 `postgres` 维护库的高权限 URL。
    database_url: String,
}

/// 独立 PostgreSQL 维护敏感 TOML 的脱敏结果。
///
/// 该类型只允许 `auctionctl postgres-rebuild` 在维护窗口短暂使用，不能并入 Public/Admin/Worker
/// 常驻配置，也不会实现序列化或暴露在 `Debug` 输出中。
#[derive(Clone)]
pub struct PostgresMaintenanceSecrets {
    database_url: SecretString,
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
pub(super) struct AppSecrets {
    pub(super) database_url: Option<SecretString>,
    pub(super) redis_url: Option<SecretString>,
    pub(super) session_signing_key: Option<SecretString>,
    pub(super) ehentai_cookie: Option<SecretString>,
    pub(super) mooglemail_cookie: Option<SecretString>,
    pub(super) public_otlp_headers: Option<SecretString>,
    pub(super) worker_otlp_headers: Option<SecretString>,
    pub(super) admin_otlp_headers: Option<SecretString>,
    pub(super) admin_actor: Option<SecretString>,
    pub(super) admin_secret: Option<SecretString>,
    pub(super) equipment_data_key_id: Option<SecretString>,
    pub(super) equipment_data_key: Option<SecretString>,
    pub(super) user_login_data_key_id: Option<SecretString>,
    pub(super) user_login_data_key: Option<SecretString>,
    pub(super) credential_data_key_id: Option<SecretString>,
    pub(super) credential_data_key: Option<SecretString>,
}

impl AppSecrets {
    pub(super) fn from_file(secret: SecretFileConfig) -> Result<Self, ConfigError> {
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
pub(super) fn load_secret_file(path: &Path) -> Result<SecretFileConfig, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|source| ConfigError::SecretRead { path: path.to_owned(), source })?;

    toml::from_str(&contents).map_err(|_| ConfigError::SecretParse { path: path.to_owned() })
}

/// 加载破坏性 PostgreSQL 运维专用的独立敏感 TOML。
///
/// 路径必须是规范化绝对路径，文件不得是 symlink，Unix 权限不得向 group/other 开放。解析错误
/// 固定脱敏，确保连接串不会进入诊断。
pub fn load_postgres_maintenance_secrets(
    path: impl AsRef<Path>,
) -> Result<PostgresMaintenanceSecrets, ConfigError> {
    const MAX_MAINTENANCE_SECRET_BYTES: u64 = 64 * 1024;
    let path = path.as_ref().to_owned();
    if !path.is_absolute() || path == Path::new("/") {
        return Err(ConfigError::InvalidMaintenanceSecretPath { path });
    }
    let canonical = fs::canonicalize(&path)
        .map_err(|source| ConfigError::SecretRead { path: path.clone(), source })?;
    if canonical != path {
        return Err(ConfigError::InvalidMaintenanceSecretPath { path });
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| ConfigError::InvalidMaintenanceSecretPath { path: path.clone() })?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|source| ConfigError::SecretRead { path: parent.to_owned(), source })?;
    if !parent_metadata.file_type().is_dir() {
        return Err(ConfigError::InvalidMaintenanceSecretPath { path });
    }
    #[cfg(unix)]
    if parent_metadata.permissions().mode() & 0o022 != 0 {
        return Err(ConfigError::InsecureMaintenanceSecretParent { path: parent.to_owned() });
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(&path)
        .map_err(|source| ConfigError::SecretRead { path: path.clone(), source })?;
    let metadata =
        file.metadata().map_err(|source| ConfigError::SecretRead { path: path.clone(), source })?;
    if !metadata.file_type().is_file() {
        return Err(ConfigError::InvalidMaintenanceSecretFile { path });
    }
    #[cfg(unix)]
    {
        // 维护 URL 可能具备删库权限；除 mode 外还要求单 hard-link 且由当前有效用户拥有，避免
        // root/共享目录外的另一个账号替换同 inode 后继续复用。
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.permissions().mode() & 0o077 != 0
            || metadata.nlink() != 1
            || metadata.uid() != effective_uid
        {
            return Err(ConfigError::InsecureMaintenanceSecretPermissions { path });
        }
    }
    if metadata.len() == 0 || metadata.len() > MAX_MAINTENANCE_SECRET_BYTES {
        return Err(ConfigError::SecretParse { path });
    }
    let mut contents = String::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_MAINTENANCE_SECRET_BYTES + 1)
        .read_to_string(&mut contents)
        .map_err(|source| ConfigError::SecretRead { path: path.clone(), source })?;
    if contents.len() as u64 > MAX_MAINTENANCE_SECRET_BYTES {
        return Err(ConfigError::SecretParse { path });
    }
    let secret: PostgresMaintenanceSecretFile =
        toml::from_str(&contents).map_err(|_| ConfigError::SecretParse { path: path.clone() })?;
    Ok(PostgresMaintenanceSecrets {
        database_url: SecretString::new("maintenance_database_url", secret.database_url)?,
    })
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

pub(super) fn load_admin_runtime_file(path: &Path) -> Result<AdminRuntimeDocument, ConfigError> {
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

pub(super) fn runtime_http_security(document: &AdminRuntimeDocument) -> HttpSecurityConfig {
    HttpSecurityConfig {
        allowed_origins: document.security.allowed_origins.clone(),
        allowed_origins_enabled: document.security.allowed_origins_enabled,
        login_rate_limit: document.security.login_rate_limit,
        mutation_rate_limit: document.security.mutation_rate_limit,
        rate_limit_window_seconds: document.security.rate_limit_window_seconds,
        cookie_secure: document.security.cookie_secure,
        request_timeout_seconds: document.security.request_timeout_seconds,
        trusted_proxy_cidrs: document.security.trusted_proxy_cidrs.clone(),
        trusted_unix_proxy: document.security.trusted_unix_proxy,
    }
}

pub(super) fn runtime_session(document: &AdminRuntimeDocument) -> SessionConfig {
    SessionConfig {
        idle_ttl_seconds: document.session.idle_ttl_seconds,
        absolute_ttl_seconds: document.session.absolute_ttl_seconds,
        max_public_sessions: document.session.max_public_sessions,
    }
}
