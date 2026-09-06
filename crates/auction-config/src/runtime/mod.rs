//! Admin runtime TOML 文件适配器。
//!
//! 该适配器只拥有非敏感 runtime 文档文件：读取要求 owner-owned regular 0640 文件，写入
//! 使用同目录临时文件、同步、rename 和目录同步，并通过独立锁文件阻止多进程 writer 交错。
//! 数据库命令账本和恢复状态仍由 `auction-storage` 持有。

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use auction_application::{
    AdminRuntimeDocument, AdminRuntimeFileError, AdminRuntimeFileStore, AdminRuntimeReplacement,
    AdminRuntimeSnapshot, ApplicationError, ApplicationFuture, MAX_ADMIN_RUNTIME_DOCUMENT_BYTES,
};

mod validation;

pub(crate) use validation::validate_runtime_document;

/// runtime 文件构造和本地持久化错误；不回显 TOML 原文。
#[derive(Debug, thiserror::Error)]
pub enum RuntimeFileStoreError {
    #[error("runtime 配置路径无效")]
    InvalidPath,
    #[error("runtime 配置文件 IO 失败：{0}")]
    Io(String),
    #[error("runtime 配置 TOML 无效")]
    Parse,
    #[error("runtime 配置文档无效：{0}")]
    InvalidDocument(String),
    #[error("runtime 配置 fingerprint 冲突")]
    Conflict,
    #[error("runtime 配置原子替换结果未知：{0}")]
    OutcomeUnknown(String),
}

/// 生产 Admin runtime TOML 文件存储。
pub struct TomlRuntimeFileStore {
    path: PathBuf,
    lock_path: PathBuf,
    mutex: Mutex<()>,
}

impl TomlRuntimeFileStore {
    /// 校验规范化绝对路径和受信任的父目录。
    pub fn new(path: impl AsRef<Path>) -> Result<Self, RuntimeFileStoreError> {
        let path = path.as_ref().to_owned();
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
            || path
                .to_string_lossy()
                .chars()
                .any(|character| matches!(character, '\0' | '\r' | '\n'))
        {
            return Err(RuntimeFileStoreError::InvalidPath);
        }
        let directory = path.parent().ok_or(RuntimeFileStoreError::InvalidPath)?;
        validate_directory(directory)?;
        Ok(Self {
            lock_path: directory.join(".admin-runtime-config.lock"),
            path,
            mutex: Mutex::new(()),
        })
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, RuntimeFileStoreError>,
    ) -> Result<T, RuntimeFileStoreError> {
        let _guard = self
            .mutex
            .lock()
            .map_err(|_| RuntimeFileStoreError::Io("runtime 文件进程内锁损坏".to_owned()))?;
        validate_directory(self.path.parent().ok_or(RuntimeFileStoreError::InvalidPath)?)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&self.lock_path)
            .map_err(io_message)?;
        if let Err(error) = lock.try_lock() {
            return Err(match error {
                std::fs::TryLockError::WouldBlock => {
                    RuntimeFileStoreError::Io("runtime 配置正由另一个进程更新".to_owned())
                }
                std::fs::TryLockError::Error(error) => io_message(error),
            });
        }
        let result = operation();
        let unlock_result = lock.unlock();
        match (result, unlock_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(io_message(error)),
        }
    }

    fn read_snapshot(&self) -> Result<AdminRuntimeSnapshot, RuntimeFileStoreError> {
        let metadata = fs::symlink_metadata(&self.path).map_err(io_message)?;
        validate_runtime_file(&metadata)?;
        let bytes = fs::read(&self.path).map_err(io_message)?;
        if bytes.is_empty() || bytes.len() > MAX_ADMIN_RUNTIME_DOCUMENT_BYTES {
            return Err(RuntimeFileStoreError::Io("runtime 配置文件大小无效".to_owned()));
        }
        let contents = String::from_utf8(bytes).map_err(|_| RuntimeFileStoreError::Parse)?;
        let document: AdminRuntimeDocument =
            toml::from_str(&contents).map_err(|_| RuntimeFileStoreError::Parse)?;
        validate_runtime_document(&document, chrono::Utc::now())
            .map_err(|error| RuntimeFileStoreError::InvalidDocument(error.to_string()))?;
        Ok(AdminRuntimeSnapshot {
            fingerprint: document.fingerprint(),
            document,
            restart_required: false,
        })
    }

    fn encode(document: &AdminRuntimeDocument) -> Result<Vec<u8>, RuntimeFileStoreError> {
        validate_runtime_document(document, chrono::Utc::now())
            .map_err(|error| RuntimeFileStoreError::InvalidDocument(error.to_string()))?;
        let encoded = toml::to_string_pretty(document).map_err(|_| RuntimeFileStoreError::Parse)?;
        let bytes = format!("{encoded}\n").into_bytes();
        if bytes.len() > MAX_ADMIN_RUNTIME_DOCUMENT_BYTES {
            return Err(RuntimeFileStoreError::Io("runtime 配置候选过大".to_owned()));
        }
        Ok(bytes)
    }

    fn atomic_write(&self, document: &AdminRuntimeDocument) -> Result<(), RuntimeFileStoreError> {
        let bytes = Self::encode(document)?;
        let current = fs::symlink_metadata(&self.path).map_err(io_message)?;
        validate_runtime_file(&current)?;
        let directory = self.path.parent().ok_or(RuntimeFileStoreError::InvalidPath)?;
        let temporary_path = directory.join(format!(
            ".admin-runtime-config.{}.{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| RuntimeFileStoreError::Io("系统时钟无效".to_owned()))?
                .as_nanos()
        ));
        let mut temporary = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary_path)
            .map_err(io_message)?;
        let result = (|| {
            temporary.write_all(&bytes).map_err(io_message)?;
            temporary.sync_all().map_err(io_message)?;
            set_private_mode(&temporary)?;
            temporary.sync_all().map_err(io_message)?;
            drop(temporary);
            fs::rename(&temporary_path, &self.path).map_err(|error| {
                RuntimeFileStoreError::OutcomeUnknown(format!("替换 runtime 配置失败：{error}"))
            })?;
            let directory_file = File::open(directory).map_err(|error| {
                RuntimeFileStoreError::OutcomeUnknown(format!("打开 runtime 配置目录失败：{error}"))
            })?;
            directory_file.sync_all().map_err(|error| {
                RuntimeFileStoreError::OutcomeUnknown(format!("同步 runtime 配置目录失败：{error}"))
            })?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }
}

impl AdminRuntimeFileStore for TomlRuntimeFileStore {
    fn validate_candidate(
        &self,
        document: &AdminRuntimeDocument,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), ApplicationError> {
        validate_runtime_document(document, now).map_err(ApplicationError::from)
    }

    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
        Box::pin(async move { self.with_lock(|| self.read_snapshot()).map_err(map_file_error) })
    }

    fn replace(
        &self,
        expected_fingerprint: String,
        document: AdminRuntimeDocument,
    ) -> ApplicationFuture<'_, AdminRuntimeReplacement> {
        Box::pin(async move {
            self.with_lock(|| {
                let current = self.read_snapshot()?;
                if current.fingerprint != expected_fingerprint {
                    return Err(RuntimeFileStoreError::Conflict);
                }
                self.atomic_write(&document)?;
                let updated = self.read_snapshot()?;
                if updated.fingerprint != document.fingerprint() {
                    return Err(RuntimeFileStoreError::OutcomeUnknown(
                        "替换后 fingerprint 不匹配".to_owned(),
                    ));
                }
                Ok(AdminRuntimeReplacement {
                    old_document: current.document,
                    old_fingerprint: current.fingerprint,
                    new_document: updated.document,
                    new_fingerprint: updated.fingerprint,
                })
            })
            .map_err(map_file_error)
        })
    }

    fn restore(&self, replacement: AdminRuntimeReplacement) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            self.with_lock(|| {
                let current = self.read_snapshot()?;
                if current.fingerprint != replacement.new_fingerprint {
                    return Err(RuntimeFileStoreError::Conflict);
                }
                self.atomic_write(&replacement.old_document)?;
                let restored = self.read_snapshot()?;
                if restored.fingerprint != replacement.old_fingerprint {
                    return Err(RuntimeFileStoreError::OutcomeUnknown(
                        "恢复后 fingerprint 不匹配".to_owned(),
                    ));
                }
                Ok(())
            })
            .map_err(map_file_error)
        })
    }
}

fn map_file_error(error: RuntimeFileStoreError) -> ApplicationError {
    let mapped = match error {
        RuntimeFileStoreError::Conflict => AdminRuntimeFileError::Conflict,
        RuntimeFileStoreError::OutcomeUnknown(_message) => AdminRuntimeFileError::OutcomeUnknown,
        other => AdminRuntimeFileError::Unavailable(other.to_string()),
    };
    ApplicationError::from(mapped)
}

fn validate_directory(path: &Path) -> Result<(), RuntimeFileStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(io_message)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(RuntimeFileStoreError::InvalidPath);
    }
    #[cfg(unix)]
    if metadata.mode() & 0o022 != 0 {
        return Err(RuntimeFileStoreError::InvalidPath);
    }
    Ok(())
}

fn validate_runtime_file(metadata: &fs::Metadata) -> Result<(), RuntimeFileStoreError> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(RuntimeFileStoreError::Io("runtime 配置必须是 regular 文件".to_owned()));
    }
    #[cfg(unix)]
    if metadata.mode() & 0o777 != 0o640 {
        return Err(RuntimeFileStoreError::Io("runtime 配置必须使用 0640 权限".to_owned()));
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_mode(file: &File) -> Result<(), RuntimeFileStoreError> {
    file.set_permissions(fs::Permissions::from_mode(0o640)).map_err(io_message)
}

#[cfg(not(unix))]
fn set_private_mode(_file: &File) -> Result<(), RuntimeFileStoreError> {
    Ok(())
}

fn io_message(error: io::Error) -> RuntimeFileStoreError {
    RuntimeFileStoreError::Io(error.to_string())
}
