//! Runtime TOML 文件的私有边界检查与读取；验证后从同一描述符读取有限大小内容。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use auction_application::admin::MAX_ADMIN_RUNTIME_DOCUMENT_BYTES;
use libc::{O_NOFOLLOW, O_NONBLOCK};

use super::RuntimeFileStoreError;

/// 启动与管理读取共用文件边界；只有管理读写要求当前进程 UID。
/// 角色配置也由 root 运维读取，目标服务 owner 仍由部署门禁按实际 unit User 校验。
pub(crate) fn read_runtime_file(
    path: &Path,
    expected_uid: Option<u32>,
) -> Result<String, RuntimeFileStoreError> {
    validate_directory(path.parent().ok_or(RuntimeFileStoreError::InvalidPath)?)?;
    // 在同一描述符上校验和读取，避免路径在 metadata 与 open 之间被替换；
    // O_NONBLOCK 让误放的 FIFO 也能在文件类型检查时立即失败。
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .map_err(io_message)?;
    let metadata = file.metadata().map_err(io_message)?;
    validate_runtime_file(&metadata, expected_uid)?;
    if metadata.len() == 0 || metadata.len() > MAX_ADMIN_RUNTIME_DOCUMENT_BYTES as u64 {
        return Err(RuntimeFileStoreError::Io("runtime 配置文件大小无效".to_owned()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    // 限制实际读取量，而非读完才检查；并发追加不能突破既有 4 MiB 文档预算。
    file.take(MAX_ADMIN_RUNTIME_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_message)?;
    if bytes.is_empty() || bytes.len() > MAX_ADMIN_RUNTIME_DOCUMENT_BYTES {
        return Err(RuntimeFileStoreError::Io("runtime 配置文件大小无效".to_owned()));
    }
    String::from_utf8(bytes).map_err(|_| RuntimeFileStoreError::Parse)
}

pub(super) fn validate_directory(path: &Path) -> Result<(), RuntimeFileStoreError> {
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

pub(super) fn validate_runtime_file(
    metadata: &fs::Metadata,
    expected_uid: Option<u32>,
) -> Result<(), RuntimeFileStoreError> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(RuntimeFileStoreError::Io("runtime 配置必须是 regular 文件".to_owned()));
    }
    #[cfg(unix)]
    {
        if metadata.mode() & 0o777 != 0o640 {
            return Err(RuntimeFileStoreError::Io("runtime 配置必须使用 0640 权限".to_owned()));
        }
        if expected_uid.is_some_and(|uid| metadata.uid() != uid) {
            return Err(RuntimeFileStoreError::Io(
                "runtime 配置必须由管理进程运行账号拥有".to_owned(),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn set_private_mode(file: &File) -> Result<(), RuntimeFileStoreError> {
    file.set_permissions(fs::Permissions::from_mode(0o640)).map_err(io_message)
}

#[cfg(not(unix))]
pub(super) fn set_private_mode(_file: &File) -> Result<(), RuntimeFileStoreError> {
    Ok(())
}

pub(super) fn io_message(error: io::Error) -> RuntimeFileStoreError {
    RuntimeFileStoreError::Io(error.to_string())
}
