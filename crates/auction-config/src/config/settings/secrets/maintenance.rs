//! PostgreSQL 高权限敏感文件的安全读取：维护凭据（建库/重建）与 owner 凭据（迁移/恢复）。

use std::{fs, io::Read, path::Path};

use serde::de::DeserializeOwned;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use super::super::ConfigError;
use super::database::resolve_database_url;
use super::model::{
    PostgresMaintenanceSecretFile, PostgresMaintenanceSecrets, PostgresOwnerSecretFile,
    PostgresOwnerSecrets,
};

/// 加载破坏性 PostgreSQL 运维专用的独立敏感 TOML（维护库 postgres 的高权限账号）。
pub fn load_postgres_maintenance_secrets(
    path: impl AsRef<Path>,
) -> Result<PostgresMaintenanceSecrets, ConfigError> {
    let secret: PostgresMaintenanceSecretFile = load_privileged_secret_file(path)?;
    Ok(PostgresMaintenanceSecrets {
        database_url: resolved_url(secret.database, secret.database_url)?,
    })
}

/// 加载迁移 owner 专用的独立敏感 TOML；只有 `auction-cli` 的建库、迁移、重建与恢复读取它。
/// 常驻角色不得引用该文件，服务账号只来自各自的主敏感 TOML。
pub fn load_postgres_owner_secrets(
    path: impl AsRef<Path>,
) -> Result<PostgresOwnerSecrets, ConfigError> {
    let secret: PostgresOwnerSecretFile = load_privileged_secret_file(path)?;
    Ok(PostgresOwnerSecrets { database_url: resolved_url(secret.database, secret.database_url)? })
}

fn resolved_url(
    database: Option<super::database::PostgresConnectionConfig>,
    database_url: Option<String>,
) -> Result<super::model::SecretString, ConfigError> {
    resolve_database_url(database, database_url)?
        .ok_or(ConfigError::InvalidDatabaseConfig("database"))
}

/// 路径必须是规范化绝对路径，文件不得是 symlink，Unix 权限不得向 group/other 开放。解析错误
/// 固定脱敏，确保连接串不会进入诊断。两类高权限文件共用同一门禁，避免 owner 文件弱于维护文件。
fn load_privileged_secret_file<T: DeserializeOwned>(
    path: impl AsRef<Path>,
) -> Result<T, ConfigError> {
    const MAX_PRIVILEGED_SECRET_BYTES: u64 = 64 * 1024;
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
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
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
        // 这些 URL 可能具备建库、删库或 DDL 权限；除 mode 外还要求单 hard-link 且由当前有效用户
        // 拥有，避免 root/共享目录外的另一个账号替换同 inode 后继续复用。
        // SAFETY: geteuid 不接收指针，也不访问 Rust 管理的内存，只返回当前进程的有效 UID。
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.permissions().mode() & 0o077 != 0
            || metadata.nlink() != 1
            || metadata.uid() != effective_uid
        {
            return Err(ConfigError::InsecureMaintenanceSecretPermissions { path });
        }
    }
    if metadata.len() == 0 || metadata.len() > MAX_PRIVILEGED_SECRET_BYTES {
        return Err(ConfigError::SecretParse { path });
    }
    let mut contents = String::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_PRIVILEGED_SECRET_BYTES + 1)
        .read_to_string(&mut contents)
        .map_err(|source| ConfigError::SecretRead { path: path.clone(), source })?;
    if contents.len() as u64 > MAX_PRIVILEGED_SECRET_BYTES {
        return Err(ConfigError::SecretParse { path });
    }
    toml::from_str(&contents).map_err(|_| ConfigError::SecretParse { path })
}
