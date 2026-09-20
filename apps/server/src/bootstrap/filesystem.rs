//! 同机 supervisor 所有权及私有运行目录；不修改用户已有配置。

use anyhow::{Context, bail};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub(super) fn config_path(input: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if input.is_absolute() {
        input.to_path_buf()
    } else {
        std::env::current_dir()?.join(input)
    };
    if absolute.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        bail!("config directory must not contain parent traversal");
    }
    let parent = absolute.parent().context("config directory requires a parent")?.canonicalize()?;
    trusted_directory(&parent)?;
    Ok(parent.join(absolute.file_name().context("config directory requires a filename")?))
}

pub(super) fn trusted_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY：只读取当前有效 UID。
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        bail!(
            "directory must be owned by the service account and not group/world writable: {}",
            path.display()
        );
    }
    Ok(())
}

pub(super) fn acquire_runtime(path: &Path) -> anyhow::Result<File> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.into()),
    }
    trusted_directory(path)?;
    if fs::metadata(path)?.mode() & 0o077 != 0 {
        bail!("server runtime directory must be owner-only");
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path.join("owner.lock"))?;
    let metadata = file.metadata()?;
    // SAFETY：只读取当前有效 UID。
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        bail!("unsafe server ownership lock");
    }
    file.try_lock().context("another supervisor or maintenance operation owns this deployment")?;
    Ok(file)
}

pub(super) fn current_group() -> anyhow::Result<String> {
    let mut group = std::mem::MaybeUninit::<libc::group>::uninit();
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0_u8; 65536];
    // SAFETY：缓冲区在调用期间可写且存活；只在成功且非空时读取 libc 返回的组名。
    unsafe {
        if libc::getgrgid_r(
            libc::getegid(),
            group.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        ) != 0
            || result.is_null()
        {
            bail!("cannot resolve current service group");
        }
        Ok(std::ffi::CStr::from_ptr((*result).gr_name).to_str()?.to_owned())
    }
}
