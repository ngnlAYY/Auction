//! Admin runtime TOML 文件适配器；只拥有非敏感文档，读取要求 owner-owned regular 0640
//! 文件，写入使用临时文件、rename 与目录同步，并由独立锁文件阻止多 writer 交错。
//! 数据库命令账本和恢复状态仍由 `auction-storage` 持有。

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use auction_application::admin::{
    AdminRuntimeAlertsConfig, AdminRuntimeConfigError, AdminRuntimeDocument, AdminRuntimeFileError,
    AdminRuntimeFileStore, AdminRuntimeMailDeliveryBudget, AdminRuntimeReplacement,
    AdminRuntimeSnapshot, MAX_ADMIN_RUNTIME_DOCUMENT_BYTES, WorkerColdConfigDigest,
};
use auction_application::core::ApplicationError;
use auction_application::core::ApplicationFuture;
use auction_application::core::RequestHash;

use self::files::{io_message, set_private_mode, validate_directory, validate_runtime_file};
use self::validation::{RuntimeWorkerOverrides, validate_sender_allowlists};
use crate::settings::{ConfigError, RuntimeWorkerConfigSource};

mod files;
mod hot;
mod validation;

pub(crate) use files::read_runtime_file;
pub use hot::{RuntimeFileStamp, load_worker_hot_config, worker_hot_config};
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

/// 覆盖一次 runtime 更新/恢复流程的跨进程锁；离开作用域时自动释放 OS 文件锁。
pub struct TomlRuntimeWorkflowGuard {
    lock: File,
    lock_path: PathBuf,
}

impl Drop for TomlRuntimeWorkflowGuard {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

/// 生产 Admin runtime TOML 文件存储。
pub struct TomlRuntimeFileStore {
    path: PathBuf,
    lock_path: PathBuf,
    tracing_headers_configured: bool,
    worker: Option<RuntimeWorkerBaseline>,
}

struct RuntimeWorkerBaseline {
    source: RuntimeWorkerConfigSource,
    fingerprint: RequestHash,
    mail_delivery: AdminRuntimeMailDeliveryBudget,
}

impl TomlRuntimeFileStore {
    /// 校验规范化绝对路径和受信任的父目录；只保留 OTLP 头存在性与 Worker 非敏感校验基值。
    pub fn new(
        path: impl AsRef<Path>,
        tracing_headers_configured: bool,
        worker_source: Option<RuntimeWorkerConfigSource>,
    ) -> Result<Self, RuntimeFileStoreError> {
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
        let worker = worker_source
            .map(|source| {
                let file = source.read()?;
                let fingerprint = file.fingerprint();
                let deployment = file.parse(&path)?;
                Ok(RuntimeWorkerBaseline {
                    source,
                    fingerprint,
                    mail_delivery: deployment.mail_delivery_budget(),
                })
            })
            .transpose()
            .map_err(|error: ConfigError| {
                RuntimeFileStoreError::InvalidDocument(error.to_string())
            })?;
        Ok(Self {
            lock_path: directory.join(".admin-runtime-config.lock"),
            path,
            tracing_headers_configured,
            worker,
        })
    }

    /// 首次部署在私有临时目录中复用正式读取校验；发布前不创建运行时锁文件。
    #[cfg(unix)]
    pub(crate) fn validate_initial_file(path: &Path) -> Result<(), RuntimeFileStoreError> {
        Self::new(path, false, None)?.read_snapshot().map(|_| ())
    }

    fn open_lock(&self) -> Result<File, RuntimeFileStoreError> {
        validate_directory(self.path.parent().ok_or(RuntimeFileStoreError::InvalidPath)?)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.lock_path)
            .map_err(io_message)?;
        let metadata = lock.metadata().map_err(io_message)?;
        // 锁必须固定在私有普通 inode 上；不能沿 symlink 或 hard link 锁住无关文件。
        // SAFETY：geteuid 不接收指针，仅返回当前进程有效 UID。
        let effective_uid = unsafe { libc::geteuid() };
        if !metadata.is_file()
            || metadata.uid() != effective_uid
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(RuntimeFileStoreError::Io(
                "runtime 锁必须是运行账号拥有的私有单链接普通文件".to_owned(),
            ));
        }
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => {
                RuntimeFileStoreError::Io("runtime 配置正由另一个进程更新".to_owned())
            }
            std::fs::TryLockError::Error(error) => io_message(error),
        })?;
        Ok(lock)
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, RuntimeFileStoreError>,
    ) -> Result<T, RuntimeFileStoreError> {
        let lock = self.open_lock()?;
        let result = operation();
        let unlock_result = lock.unlock();
        match (result, unlock_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(io_message(error)),
        }
    }

    fn read_snapshot(&self) -> Result<AdminRuntimeSnapshot, RuntimeFileStoreError> {
        // SAFETY：geteuid 不接收指针，仅返回当前进程有效 UID。
        let owner_uid = unsafe { libc::geteuid() };
        let contents = read_runtime_file(&self.path, Some(owner_uid))?;
        let document: AdminRuntimeDocument =
            toml::from_str(&contents).map_err(|_| RuntimeFileStoreError::Parse)?;
        self.validate_document(&document, auction_time::Timestamp::now())
            .map_err(|error| RuntimeFileStoreError::InvalidDocument(error.to_string()))?;
        Ok(AdminRuntimeSnapshot {
            fingerprint: document.fingerprint(),
            document,
            restart_required: false,
        })
    }

    fn validate_document(
        &self,
        document: &AdminRuntimeDocument,
        now: auction_time::Timestamp,
    ) -> Result<(), AdminRuntimeConfigError> {
        validate_runtime_document(document, now)?;
        // headers 只由敏感部署文件拥有；不能保存一个重启后必因遗留 headers 而失败的关闭候选。
        if !document.tracing.enabled && self.tracing_headers_configured {
            return Err(AdminRuntimeConfigError::InvalidDocument(
                "关闭 tracing 前须先移除敏感 TOML 的 admin_otlp_headers 并重启 Admin".to_owned(),
            ));
        }
        Ok(())
    }

    fn encode(&self, document: &AdminRuntimeDocument) -> Result<Vec<u8>, RuntimeFileStoreError> {
        self.validate_document(document, auction_time::Timestamp::now())
            .map_err(|error| RuntimeFileStoreError::InvalidDocument(error.to_string()))?;
        let mut encoded =
            toml::to_string_pretty(document).map_err(|_| RuntimeFileStoreError::Parse)?;
        encoded.push('\n');
        let bytes = encoded.into_bytes();
        if bytes.len() > MAX_ADMIN_RUNTIME_DOCUMENT_BYTES {
            return Err(RuntimeFileStoreError::Io("runtime 配置候选过大".to_owned()));
        }
        Ok(bytes)
    }

    fn atomic_write(&self, document: &AdminRuntimeDocument) -> Result<(), RuntimeFileStoreError> {
        let bytes = self.encode(document)?;
        let directory = self.path.parent().ok_or(RuntimeFileStoreError::InvalidPath)?;
        let mut directory_options = OpenOptions::new();
        directory_options.read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        let directory_file = directory_options.open(directory).map_err(io_message)?;
        let directory_metadata = directory_file.metadata().map_err(io_message)?;
        if !directory_metadata.is_dir() {
            return Err(RuntimeFileStoreError::InvalidPath);
        }
        #[cfg(unix)]
        if directory_metadata.mode() & 0o022 != 0 {
            return Err(RuntimeFileStoreError::InvalidPath);
        }
        let stable_directory =
            PathBuf::from(format!("/proc/self/fd/{}", directory_file.as_raw_fd()));
        let name = self.path.file_name().ok_or(RuntimeFileStoreError::InvalidPath)?;
        let stable_path = stable_directory.join(name);
        let current = fs::symlink_metadata(&stable_path).map_err(io_message)?;
        // SAFETY：geteuid 不接收指针，仅返回当前进程有效 UID。
        let owner_uid = unsafe { libc::geteuid() };
        validate_runtime_file(&current, Some(owner_uid))?;
        let temporary_path = stable_directory.join(format!(
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
            set_private_mode(&temporary)?;
            // 一次 fsync 同时持久化内容与最终权限，完成后才允许 rename。
            temporary.sync_all().map_err(io_message)?;
            drop(temporary);
            let latest = fs::symlink_metadata(&stable_path).map_err(io_message)?;
            validate_runtime_file(&latest, Some(owner_uid))?;
            #[cfg(unix)]
            if latest.dev() != current.dev() || latest.ino() != current.ino() {
                return Err(RuntimeFileStoreError::Io(
                    "runtime 配置文件在替换前发生变化".to_owned(),
                ));
            }
            fs::rename(&temporary_path, &stable_path).map_err(|error| {
                RuntimeFileStoreError::OutcomeUnknown(format!("替换 runtime 配置失败：{error}"))
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

    fn replace_unlocked(
        &self,
        expected_fingerprint: String,
        document: AdminRuntimeDocument,
    ) -> Result<AdminRuntimeReplacement, RuntimeFileStoreError> {
        let current = self.read_snapshot()?;
        if current.fingerprint != expected_fingerprint {
            return Err(RuntimeFileStoreError::Conflict);
        }
        self.validate_candidate_against(&current, &document, auction_time::Timestamp::now())
            .map_err(|error| RuntimeFileStoreError::InvalidDocument(error.to_string()))?;
        self.atomic_write(&document)?;
        // 原子替换已完成；回读失败不能伪装成文件尚未变更的普通读取错误。
        let updated = self
            .read_snapshot()
            .map_err(|error| RuntimeFileStoreError::OutcomeUnknown(error.to_string()))?;
        if updated.fingerprint != document.fingerprint() {
            return Err(RuntimeFileStoreError::OutcomeUnknown(
                "替换后 fingerprint 不匹配".to_owned(),
            ));
        }
        Ok(AdminRuntimeReplacement {
            old_fingerprint: current.fingerprint,
            new_fingerprint: updated.fingerprint,
        })
    }

    fn ensure_workflow_guard(
        &self,
        guard: &TomlRuntimeWorkflowGuard,
    ) -> Result<(), RuntimeFileStoreError> {
        if guard.lock_path != self.lock_path {
            return Err(RuntimeFileStoreError::Io(
                "runtime workflow 锁不属于当前配置文件".to_owned(),
            ));
        }
        Ok(())
    }
}

impl AdminRuntimeFileStore for TomlRuntimeFileStore {
    type WorkflowGuard = TomlRuntimeWorkflowGuard;

    fn acquire_workflow_lock(&self) -> ApplicationFuture<'_, Self::WorkflowGuard> {
        Box::pin(async move {
            self.open_lock()
                .map(|lock| TomlRuntimeWorkflowGuard { lock, lock_path: self.lock_path.clone() })
                .map_err(map_file_error)
        })
    }

    fn validate_candidate(
        &self,
        document: &AdminRuntimeDocument,
        now: auction_time::Timestamp,
    ) -> Result<(), ApplicationError> {
        let _lock = self.open_lock().map_err(map_file_error)?;
        let current = self.read_snapshot().map_err(map_file_error)?;
        self.validate_candidate_against(&current, document, now)
    }

    fn validate_candidate_against(
        &self,
        current: &AdminRuntimeSnapshot,
        document: &AdminRuntimeDocument,
        now: auction_time::Timestamp,
    ) -> Result<(), ApplicationError> {
        self.validate_document(document, now).map_err(ApplicationError::from)?;
        validate_sender_allowlists(document)?;
        if let Some(operations) = &document.operations {
            operations.validate_mail_delivery_budget(
                self.worker.as_ref().map(|base| base.mail_delivery),
            )?;
        }
        let invalid = |reason: &str| AdminRuntimeConfigError::InvalidDocument(reason.to_owned());
        // 有可信部署基值时，登记命令前总要验证最终候选，避免无关编辑把已知的启动错误再次写入。
        // 只有拿不到基值时，才按同一锁内快照判断能否沿用无关编辑的兼容路径。
        let unavailable = |reason: &str| {
            if RuntimeWorkerOverrides::from_document(&current.document)
                == RuntimeWorkerOverrides::from_document(document)
            {
                Ok(())
            } else {
                Err(invalid(reason).into())
            }
        };
        let Some(worker) = &self.worker else {
            return unavailable(AdminRuntimeAlertsConfig::WORKER_CONFIG_REQUIRED_HINT);
        };
        // 登记命令前先核对原文指纹；内容损坏也属于部署变更，解析使用刚通过 fence 的同一份原文。
        let Ok(file) = worker.source.read() else {
            return unavailable(AdminRuntimeAlertsConfig::WORKER_CONFIG_UNREADABLE_HINT);
        };
        if file.fingerprint() != worker.fingerprint {
            return unavailable(AdminRuntimeAlertsConfig::WORKER_CONFIG_CHANGED_HINT);
        }
        let result = file
            .parse(&self.path)
            .and_then(|deployment| deployment.validate_runtime(document.clone(), now));
        match result {
            Ok(()) => Ok(()),
            Err(ConfigError::Read { .. } | ConfigError::SecretRead { .. }) => {
                unavailable("候选未通过 Worker 启动配置校验")
            }
            Err(error) => Err(invalid(match error {
                ConfigError::InvalidOperationsAlertConfig(reason)
                | ConfigError::InvalidMoogleMailConfig(reason)
                | ConfigError::InvalidAuctionConfig(reason) => reason,
                _ => "候选未通过 Worker 启动配置校验",
            })
            .into()),
        }
    }

    fn worker_cold_config_target(
        &self,
        snapshot: &AdminRuntimeSnapshot,
    ) -> Option<WorkerColdConfigDigest> {
        let worker = self.worker.as_ref()?;
        let file = worker.source.read().ok()?;
        // 沿用候选校验的部署指纹围栏；目标计算只消费 GET 本次读到的文档，不打开 Worker 密钥文件。
        if file.fingerprint() != worker.fingerprint {
            return None;
        }
        file.parse(&self.path).ok()?.cold_config_target(snapshot.document.clone()).ok()
    }

    fn current(&self) -> ApplicationFuture<'_, AdminRuntimeSnapshot> {
        Box::pin(async move { self.with_lock(|| self.read_snapshot()).map_err(map_file_error) })
    }

    fn current_locked<'a>(
        &'a self,
        guard: &'a Self::WorkflowGuard,
    ) -> ApplicationFuture<'a, AdminRuntimeSnapshot> {
        Box::pin(async move {
            self.ensure_workflow_guard(guard).map_err(map_file_error)?;
            self.read_snapshot().map_err(map_file_error)
        })
    }

    fn replace_locked<'a>(
        &'a self,
        guard: &'a Self::WorkflowGuard,
        expected_fingerprint: String,
        document: AdminRuntimeDocument,
    ) -> ApplicationFuture<'a, AdminRuntimeReplacement> {
        Box::pin(async move {
            self.ensure_workflow_guard(guard).map_err(map_file_error)?;
            self.replace_unlocked(expected_fingerprint, document).map_err(map_file_error)
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
