//! Unix 私有目录、跨进程锁和无覆盖发布；所有阻塞操作由归档层调度。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{
    collections::VecDeque,
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

use auction_application::MailDiagnosticError;
use uuid::Uuid;

use super::{MAX_FILE_BYTES, MoogleMailDiagnosticArchiveConfig};

const HARD_MAX_FILES: usize = 1024;

struct LockedDirectory {
    path: PathBuf,
    owner: u32,
    _lock: File,
}

/// 目录配置可相对于 Worker TOML；逐级拒绝符号链接后才规范化父目录。
fn normalized_target(target: &Path) -> Result<PathBuf, MailDiagnosticError> {
    let target = std::path::absolute(target).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "resolve_absolute_path",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    let name = target.file_name().ok_or(MailDiagnosticError::InvalidContext)?;
    let parent = target.parent().ok_or(MailDiagnosticError::InvalidContext)?;
    let mut prefix = PathBuf::new();
    for component in parent.components() {
        prefix.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&prefix).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "inspect_parent",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(MailDiagnosticError::Io);
        }
    }
    let parent = fs::canonicalize(parent).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "canonicalize_parent",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    Ok(parent.join(name))
}

fn private_directory(
    target: &Path,
    create: bool,
) -> Result<(PathBuf, Metadata), MailDiagnosticError> {
    let path = normalized_target(target)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
            let result = DirBuilder::new().mode(0o700).create(&path);
            if result.as_ref().is_err_and(|error| error.kind() != io::ErrorKind::AlreadyExists) {
                return Err(MailDiagnosticError::Io);
            }
        }
        Err(diagnostic_cause) => {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "read_directory",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return Err(MailDiagnosticError::NotFound);
        }
    }
    let metadata = fs::symlink_metadata(&path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_directory",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(MailDiagnosticError::Io);
    }
    Ok((path, metadata))
}

fn private_file(
    path: &Path,
    owner: u32,
    allow_two_links: bool,
) -> Result<Metadata, MailDiagnosticError> {
    let metadata = fs::symlink_metadata(path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_file",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o7777 != 0o600
        || !(metadata.nlink() == 1 || allow_two_links && metadata.nlink() == 2)
    {
        return Err(MailDiagnosticError::Io);
    }
    Ok(metadata)
}

fn same_file(first: &Metadata, second: &Metadata) -> bool {
    first.dev() == second.dev() && first.ino() == second.ino()
}

fn open_private(path: &Path, owner: u32, write: bool) -> Result<File, MailDiagnosticError> {
    let before = private_file(path, owner, false)?;
    let file =
        OpenOptions::new().read(true).write(write).open(path).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "open_file",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
    let opened = file.metadata().map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_open_file",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    let after = private_file(path, owner, false)?;
    if !same_file(&before, &opened) || !same_file(&opened, &after) {
        return Err(MailDiagnosticError::Io);
    }
    Ok(file)
}

fn locked_directory(target: &Path, create: bool) -> Result<LockedDirectory, MailDiagnosticError> {
    let (path, directory_metadata) = private_directory(target, create)?;
    let owner = directory_metadata.uid();
    let lock_path = path.join(".lock");
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&lock_path)
    {
        Ok(file) => {
            let opened = file.metadata().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "inspect_lock_file",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            })?;
            if !private_file(&lock_path, owner, false)
                .is_ok_and(|current| same_file(&current, &opened))
            {
                if fs::symlink_metadata(&lock_path)
                    .is_ok_and(|current| same_file(&current, &opened))
                {
                    let _ = fs::remove_file(&lock_path);
                }
                return Err(MailDiagnosticError::Io);
            }
            file
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            open_private(&lock_path, owner, true)?
        }
        Err(diagnostic_cause) => {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "create_lock_file",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return Err(MailDiagnosticError::Io);
        }
    };
    // 不在后台阻塞任务中无限等待其他进程；锁繁忙也是可观测的留证失败。
    lock.try_lock().map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "lock_directory",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    let current = fs::symlink_metadata(&path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_locked_directory",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    if !same_file(&directory_metadata, &current) {
        return Err(MailDiagnosticError::Io);
    }

    // 新建的空私有文件由当前有效账号拥有，用它核对预存目录 owner，避免依赖环境变量或新增 FFI。
    let probe_path = path.join(format!(".{}.tmp", Uuid::new_v4()));
    let probe =
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(&probe_path).map_err(
            |diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "create_probe",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            },
        )?;
    let probe_metadata = probe.metadata().map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_probe",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    });
    drop(probe);
    fs::remove_file(&probe_path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "remove_probe",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    if probe_metadata?.uid() != owner {
        return Err(MailDiagnosticError::Io);
    }
    Ok(LockedDirectory { path, owner, _lock: lock })
}

fn managed_name(name: &str, temporary: bool) -> Option<Uuid> {
    let raw = if temporary {
        name.strip_prefix('.')?.strip_suffix(".tmp")?
    } else {
        name.strip_suffix(".evidence")?
    };
    let id = Uuid::parse_str(raw).ok()?;
    (!id.is_nil() && id.to_string() == raw).then_some(id)
}

fn remove_file(path: &Path) -> Result<(), MailDiagnosticError> {
    fs::remove_file(path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "remove_record",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })
}

/// 只清理严格命名的本模块临时文件；先收集再清理，未知文件使整次操作失败。
fn collect_records(
    directory: &LockedDirectory,
) -> Result<VecDeque<(PathBuf, SystemTime, u64)>, MailDiagnosticError> {
    let mut temporary = Vec::new();
    let mut records = Vec::new();
    for entry in fs::read_dir(&directory.path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "list_records",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })? {
        let entry = entry.map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "read_record_entry",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(MailDiagnosticError::Io)?;
        if name == ".lock" {
            private_file(&entry.path(), directory.owner, false)?;
            continue;
        }
        if let Some(id) = managed_name(name, true) {
            temporary.push((entry.path(), id));
        } else if managed_name(name, false).is_some() {
            records.push(entry.path());
        } else {
            return Err(MailDiagnosticError::Io);
        }
        if temporary.len() + records.len() > HARD_MAX_FILES * 2 + 2 {
            return Err(MailDiagnosticError::Capacity);
        }
        private_file(&entry.path(), directory.owner, true)?;
    }
    for (path, id) in temporary {
        let metadata = private_file(&path, directory.owner, true)?;
        if metadata.nlink() == 2 {
            let final_path = directory.path.join(format!("{id}.evidence"));
            let final_metadata = private_file(&final_path, directory.owner, true)?;
            if !same_file(&metadata, &final_metadata) {
                return Err(MailDiagnosticError::Io);
            }
        }
        remove_file(&path)?;
    }
    let mut result = Vec::with_capacity(records.len());
    for path in records {
        let metadata = private_file(&path, directory.owner, false)?;
        if metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(MailDiagnosticError::Capacity);
        }
        result.push((
            path,
            metadata.modified().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "read_record_timestamp",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            })?,
            metadata.len(),
        ));
    }
    result.sort_by_key(|(_, modified, _)| *modified);
    Ok(result.into())
}

pub(super) fn persist(
    config: &MoogleMailDiagnosticArchiveConfig,
    id: Uuid,
    bytes: &[u8],
) -> Result<(), MailDiagnosticError> {
    let directory = locked_directory(&config.directory, true)?;
    let destination = directory.path.join(format!("{id}.evidence"));
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        _ => return Err(MailDiagnosticError::Io),
    }
    let mut records = collect_records(&directory)?;
    let now = SystemTime::now();
    let mut total = records
        .iter()
        .try_fold(0u64, |sum, (_, _, size)| sum.checked_add(*size))
        .ok_or(MailDiagnosticError::Capacity)?;
    // 每次写入都清理过期证据；容量不足时继续淘汰最旧记录，不要求先等满保留期。
    while let Some((path, modified, size)) = records.front() {
        let expired = now.duration_since(*modified).is_ok_and(|age| age >= config.retention);
        let too_many = records.len() >= HARD_MAX_FILES;
        let too_large = total
            .checked_add(bytes.len() as u64)
            .is_none_or(|size| size > config.maximum_total_bytes);
        if !expired && !too_many && !too_large {
            break;
        }
        remove_file(path)?;
        total -= size;
        records.pop_front();
    }
    if total.checked_add(bytes.len() as u64).is_none_or(|size| size > config.maximum_total_bytes) {
        return Err(MailDiagnosticError::Capacity);
    }
    let temporary = directory.path.join(format!(".{id}.tmp"));
    publish_private(&temporary, &destination, bytes, Some(directory.owner))
}

pub(super) fn read(
    config: &MoogleMailDiagnosticArchiveConfig,
    id: Uuid,
) -> Result<Vec<u8>, MailDiagnosticError> {
    let directory = locked_directory(&config.directory, false)?;
    let path = directory.path.join(format!("{id}.evidence"));
    if fs::symlink_metadata(&path).is_err_and(|error| error.kind() == io::ErrorKind::NotFound) {
        return Err(MailDiagnosticError::NotFound);
    }
    let file = open_private(&path, directory.owner, false)?;
    if file
        .metadata()
        .map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "inspect_record",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?
        .len()
        > MAX_FILE_BYTES as u64
    {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "read_record",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(MailDiagnosticError::PayloadTooLarge);
    }
    Ok(bytes)
}

fn publish_private(
    temporary: &Path,
    destination: &Path,
    bytes: &[u8],
    owner: Option<u32>,
) -> Result<(), MailDiagnosticError> {
    let mut file =
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(temporary).map_err(
            |diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "create_private_file",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            },
        )?;
    let metadata = file.metadata().map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "inspect_private_file",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })?;
    let mut published = false;
    let result = (|| {
        if owner.is_some_and(|owner| metadata.uid() != owner)
            || metadata.permissions().mode() & 0o7777 != 0o600
        {
            return Err(MailDiagnosticError::Io);
        }
        file.write_all(bytes).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "write_record",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        file.sync_all().map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "sync_record",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        // hard_link 在目标已存在时失败，保证不会覆盖已发布证据或用户的导出文件。
        fs::hard_link(temporary, destination).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "publish_record",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        published = true;
        remove_file(temporary)?;
        let parent = destination.parent().ok_or(MailDiagnosticError::Io)?;
        File::open(parent).and_then(|file| file.sync_all()).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "sync_parent",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })
    })();
    if result.is_err() {
        for path in [Some(temporary), published.then_some(destination)].into_iter().flatten() {
            if fs::symlink_metadata(path).is_ok_and(|current| same_file(&current, &metadata)) {
                let _ = fs::remove_file(path);
            }
        }
    }
    result
}

pub(super) fn write_private_export(output: &Path, bytes: &[u8]) -> Result<(), MailDiagnosticError> {
    let output = normalized_target(output)?;
    let parent = output.parent().ok_or(MailDiagnosticError::InvalidContext)?;
    let temporary = parent.join(format!(".auction-mail-export-{}.tmp", Uuid::new_v4()));
    publish_private(&temporary, &output, bytes, None)
}
