//! Unix 私有目录、跨进程锁和无覆盖发布；所有阻塞操作由归档层调度。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use auction_application::MailDiagnosticError;
use uuid::Uuid;

use super::{
    EvictionHeader, MAX_FILE_BYTES, MAX_HEADER_BYTES, MailDiagnosticPurgeOutcome,
    MailDiagnosticRetentionSnapshot, MoogleMailDiagnosticArchiveConfig, RecordEntry,
    WRITE_UNKNOWN_FAILURE_CODE, eviction_order, eviction_plan,
};

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
fn collect_records(directory: &LockedDirectory) -> Result<Vec<RecordEntry>, MailDiagnosticError> {
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
        result.push(RecordEntry {
            path,
            modified: metadata.modified().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "read_record_timestamp",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            })?,
            size: metadata.len(),
            // 分类只在本次写入确实需要淘汰时才读取，普通追加不额外扫描证据头。
            write_unknown: false,
        });
    }
    result.sort_by_key(|record| record.modified);
    Ok(result)
}

/// 读取公开头首行判断是否为写入未知证据。
/// 无法读取或解析时按“需要保留”处理：分类失败不能成为先删证据的理由，
/// 容量边界仍由后续的统一检查兜底。
fn write_unknown_evidence(path: &Path, owner: u32) -> bool {
    let Ok(file) = open_private(path, owner, false) else {
        return true;
    };
    let mut header = Vec::new();
    if file.take((MAX_HEADER_BYTES + 1) as u64).read_to_end(&mut header).is_err() {
        return true;
    }
    let Some(split) = header.iter().position(|byte| *byte == b'\n') else {
        return true;
    };
    let Ok(parsed) = serde_json::from_slice::<EvictionHeader>(&header[..split]) else {
        return true;
    };
    parsed.outcome.failure_code == WRITE_UNKNOWN_FAILURE_CODE
}

pub(super) fn retention_snapshot(
    directory: &Path,
    retention: Duration,
    hold: Duration,
    now: SystemTime,
) -> Result<MailDiagnosticRetentionSnapshot, MailDiagnosticError> {
    let path = normalized_target(directory)?;
    let directory = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(MailDiagnosticRetentionSnapshot::default());
        }
        Err(error) => {
            let _ = error;
            return Err(MailDiagnosticError::Io);
        }
    };
    if directory.file_type().is_symlink() || !directory.is_dir() {
        return Err(MailDiagnosticError::Io);
    }
    let owner = directory.uid();
    let mut snapshot =
        MailDiagnosticRetentionSnapshot { directory_present: true, ..Default::default() };
    for entry in fs::read_dir(&path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "list_retention_snapshot",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })? {
        let entry = entry.map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "read_retention_entry",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            snapshot.unexpected_entries = snapshot.unexpected_entries.saturating_add(1);
            continue;
        };
        if managed_name(name, false).is_some() {
            let metadata = private_file(&entry.path(), owner, false)?;
            let modified = metadata.modified().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "read_retention_timestamp",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            })?;
            let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
            snapshot.total_files = snapshot.total_files.saturating_add(1);
            let write_unknown =
                if age >= retention { write_unknown_evidence(&entry.path(), owner) } else { false };
            match super::classify_evidence(age, retention, hold, write_unknown) {
                super::EvidenceRetention::Retained => {
                    snapshot.retained_files = snapshot.retained_files.saturating_add(1);
                }
                super::EvidenceRetention::OrdinaryExpired => {
                    snapshot.ordinary_expired_files =
                        snapshot.ordinary_expired_files.saturating_add(1);
                    snapshot.ordinary_expired_bytes =
                        snapshot.ordinary_expired_bytes.saturating_add(metadata.len());
                }
                super::EvidenceRetention::WriteUnknownHeld => {
                    snapshot.write_unknown_held_files =
                        snapshot.write_unknown_held_files.saturating_add(1);
                    snapshot.write_unknown_held_bytes =
                        snapshot.write_unknown_held_bytes.saturating_add(metadata.len());
                }
                super::EvidenceRetention::WriteUnknownOverHold => {
                    snapshot.write_unknown_held_files =
                        snapshot.write_unknown_held_files.saturating_add(1);
                    snapshot.write_unknown_held_bytes =
                        snapshot.write_unknown_held_bytes.saturating_add(metadata.len());
                    snapshot.write_unknown_over_hold_window_files =
                        snapshot.write_unknown_over_hold_window_files.saturating_add(1);
                }
            }
        } else if managed_name(name, true).is_some() {
            snapshot.temporary_files = snapshot.temporary_files.saturating_add(1);
        } else {
            snapshot.unexpected_entries = snapshot.unexpected_entries.saturating_add(1);
        }
    }
    Ok(snapshot)
}

pub(super) fn purge_ordinary_expired(
    directory: &Path,
    retention: Duration,
    hold: Duration,
    now: SystemTime,
    max_files: u64,
) -> Result<MailDiagnosticPurgeOutcome, MailDiagnosticError> {
    let target = normalized_target(directory)?;
    let locked = match locked_directory(&target, false) {
        Ok(locked) => locked,
        Err(MailDiagnosticError::NotFound) => {
            return Ok(MailDiagnosticPurgeOutcome::default());
        }
        Err(error) => return Err(error),
    };
    let owner = locked.owner;
    let mut outcome = MailDiagnosticPurgeOutcome {
        directory_present: true,
        ..MailDiagnosticPurgeOutcome::default()
    };
    let mut candidates: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    for entry in fs::read_dir(&locked.path).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "mooglemail_diagnostics",
            "list_purge_entries",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        MailDiagnosticError::Io
    })? {
        let entry = entry.map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "mooglemail_diagnostics",
                "read_purge_entry",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            MailDiagnosticError::Io
        })?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            outcome.unexpected_entries = outcome.unexpected_entries.saturating_add(1);
            continue;
        };
        if name == ".lock" {
            // 本模块自己的目录锁；不是证据文件。
            continue;
        }
        if managed_name(name, false).is_some() {
            let metadata = private_file(&entry.path(), owner, false)?;
            let modified = metadata.modified().map_err(|diagnostic_cause| {
                safe_diagnostics::error(
                    "mooglemail_diagnostics",
                    "read_purge_timestamp",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                MailDiagnosticError::Io
            })?;
            let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
            outcome.examined_files = outcome.examined_files.saturating_add(1);
            let write_unknown =
                if age >= retention { write_unknown_evidence(&entry.path(), owner) } else { false };
            match super::classify_evidence(age, retention, hold, write_unknown) {
                super::EvidenceRetention::Retained => {
                    outcome.retained_files = outcome.retained_files.saturating_add(1);
                }
                super::EvidenceRetention::OrdinaryExpired => {
                    candidates.push((entry.path(), modified, metadata.len()));
                }
                super::EvidenceRetention::WriteUnknownHeld => {
                    outcome.write_unknown_held_files =
                        outcome.write_unknown_held_files.saturating_add(1);
                }
                super::EvidenceRetention::WriteUnknownOverHold => {
                    outcome.write_unknown_held_files =
                        outcome.write_unknown_held_files.saturating_add(1);
                    outcome.write_unknown_over_hold_window_files =
                        outcome.write_unknown_over_hold_window_files.saturating_add(1);
                    outcome.needs_follow_up = true;
                }
            }
        } else if managed_name(name, true).is_some() {
            // 临时文件可能属于在途写入；不计数也不删除。
        } else {
            outcome.unexpected_entries = outcome.unexpected_entries.saturating_add(1);
        }
    }
    if outcome.unexpected_entries > 0 {
        outcome.needs_follow_up = true;
    }
    if candidates.len() as u64 > max_files {
        outcome.limit_reached = true;
    }
    // 先按修改时间从旧到新排序，再按批准上限截断：超限时必须删除最旧的候选。
    candidates.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    candidates.truncate(usize::try_from(max_files).unwrap_or(usize::MAX));
    for (path, _, _) in &candidates {
        let before = match private_file(path, owner, false) {
            Ok(metadata) => metadata,
            Err(error) => {
                let _ = error;
                outcome.failure_code = Some("evidence_file_changed");
                break;
            }
        };
        match fs::remove_file(path) {
            Ok(()) => {
                outcome.deleted_files = outcome.deleted_files.saturating_add(1);
                outcome.deleted_bytes = outcome.deleted_bytes.saturating_add(before.len());
            }
            Err(error) => {
                let _ = error;
                outcome.failure_code = Some("evidence_delete_failed");
                break;
            }
        }
    }
    if outcome.failure_code.is_none()
        && let Err(error) = File::open(&locked.path).and_then(|directory| directory.sync_all())
    {
        let _ = error;
        outcome.failure_code = Some("evidence_directory_sync_failed");
    }
    Ok(outcome)
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
    let incoming = bytes.len() as u64;
    let plan = |records: &[RecordEntry]| {
        eviction_plan(
            records,
            now,
            incoming,
            config.retention,
            config.maximum_total_bytes,
            HARD_MAX_FILES,
        )
    };
    // 普通追加不需要读取证据头：没有淘汰需求时直接用修改时间顺序。
    let mut evicted = plan(&records)?;
    if evicted > 0 {
        // 确实需要淘汰时才读取公开头分类，把写入未知证据排到普通证据之后。
        for record in &mut records {
            record.write_unknown = write_unknown_evidence(&record.path, directory.owner);
        }
        records = eviction_order(records).into();
        evicted = plan(&records)?;
    }
    for record in records.iter().take(evicted) {
        remove_file(&record.path)?;
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
