//! 合并部署的显式迁移入口；与 supervisor 及其所有子进程共享同一文件锁。

use super::*;
use std::{
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
};

pub(super) fn guard(config: &AppConfig) -> anyhow::Result<Option<File>> {
    let config_dir = config
        .config_path
        .as_ref()
        .and_then(|path| path.parent())
        .context("explicit configuration directory required")?
        .canonicalize()?;
    let runtime = auction_config::managed_runtime_directory(&config_dir)?;
    // 固定目录描述符后再相对打开锁，防止两次路径解析之间换成另一目录。
    let directory = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&runtime)
    {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("open managed runtime directory"),
    };
    let directory_metadata = directory.metadata()?;
    // SAFETY：只读取运维调用者有效 UID。
    if !directory_metadata.is_dir()
        || directory_metadata.uid() != unsafe { libc::geteuid() }
        || directory_metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("managed runtime directory must be owner-only and owned by this account");
    }
    // SAFETY：目录描述符在调用期间有效；路径为固定 NUL 结尾字面量，不创建文件。
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c"owner.lock".as_ptr(),
            libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error()).context("open managed deployment lock");
    }
    // SAFETY：openat 成功返回独占所有权的新描述符，由 File 负责关闭。
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file.metadata()?;
    // SAFETY：锁必须与当前有效 UID 一致，避免维护与服务使用不同身份。
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        anyhow::bail!("invalid managed deployment ownership lock");
    }
    file.try_lock()
        .context("stop auction-server and wait for all child processes before migration")?;
    // 加锁后核对路径仍指向已打开的目录与锁 inode；不把被替换的路径当作原 supervisor。
    let current_directory = fs::symlink_metadata(&runtime)?;
    let current_lock = fs::symlink_metadata(runtime.join("owner.lock"))?;
    if !current_directory.is_dir()
        || current_directory.dev() != directory_metadata.dev()
        || current_directory.ino() != directory_metadata.ino()
        || current_directory.uid() != directory_metadata.uid()
        || current_directory.mode() & 0o077 != 0
        || !current_lock.is_file()
        || current_lock.dev() != metadata.dev()
        || current_lock.ino() != metadata.ino()
        || current_lock.uid() != metadata.uid()
        || current_lock.nlink() != 1
        || current_lock.mode() & 0o077 != 0
    {
        anyhow::bail!("managed runtime directory or ownership lock changed during acquisition");
    }
    Ok(Some(file))
}

pub(super) async fn migrate(config_dir: &Path) -> anyhow::Result<()> {
    if !config_dir.is_absolute() {
        anyhow::bail!("managed config directory must be absolute");
    }
    let [config, _, _] = deployment_check(config_dir, true)?;
    // 必须由同一次实际加锁证明合并部署存在，不能先检查标记再接受缺失的可选锁。
    dispatch::migrate_managed_config(&config).await
}
