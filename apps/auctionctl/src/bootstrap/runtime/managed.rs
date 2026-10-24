//! 合并部署的显式迁移入口；与 supervisor 及其所有子进程共享同一文件锁。

use super::*;
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

pub(super) fn guard(config: &AppConfig) -> anyhow::Result<Option<File>> {
    let config_dir = config
        .config_path
        .as_ref()
        .and_then(|path| path.parent())
        .context("explicit configuration directory required")?
        .canonicalize()?;
    let runtime = auction_config::managed_runtime_directory(&config_dir)?;
    match fs::symlink_metadata(&runtime) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) => {
            // SAFETY：只读取运维调用者有效 UID。
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
            {
                anyhow::bail!(
                    "managed runtime directory must be owner-only and owned by this account"
                );
            }
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(runtime.join("owner.lock"))?;
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
    Ok(Some(file))
}

pub(super) async fn migrate(config_dir: &Path) -> anyhow::Result<()> {
    if !config_dir.is_absolute() {
        anyhow::bail!("managed config directory must be absolute");
    }
    let [config, _, _] = deployment_check(config_dir, true)?;
    // 缺少合并部署标记不允许此命令绕过分体 systemd 停止门禁。
    let runtime = auction_config::managed_runtime_directory(&config_dir.canonicalize()?)?;
    if !runtime.join("owner.lock").try_exists()? {
        anyhow::bail!("not an initialized managed deployment");
    }
    migrate_config(&config).await
}
