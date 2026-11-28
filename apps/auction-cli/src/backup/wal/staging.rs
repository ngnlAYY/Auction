//! WAL 归档、每周基础备份与上传器共用的暂存发布：固定权限、有界 stderr、发布证据 manifest
//! 与可重写的本机证据。暂存目录由 PostgreSQL 账号写入、上传账号读取，权限按组读设计。

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::Context;

use crate::support::files;
use crate::support::process;

/// 子进程 stderr 的有界捕获上限；失败原因进入 PostgreSQL 或 systemd 日志，不进入回执文件。
pub(super) const STDERR_LIMIT: usize = 4096;
/// 密文与 manifest 允许同组读取：归档/基础备份账号（postgres）与上传账号可能不同。
pub(super) const STAGED_MODE: u32 = 0o640;

/// 固定临时名 `<stem><suffix>.tmp`：同一身份的重试必须先清理或复用上次残留，避免
/// create_new 永久失败。
pub(super) fn temp_path(directory: &Path, stem: &str, suffix: &str) -> PathBuf {
    directory.join(format!("{stem}{suffix}.tmp"))
}

/// 本次调用拥有的临时名在取消或失败时尽力回收；成功发布后显式解除守卫。
pub(super) struct TempPathGuard {
    path: PathBuf,
    armed: bool,
}

impl TempPathGuard {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempPathGuard {
    fn drop(&mut self) {
        if self.armed
            && fs::remove_file(&self.path).is_ok()
            && let Some(parent) = self.path.parent()
        {
            let _ = files::sync_directory(parent, "暂存目录");
        }
    }
}

/// manifest 是发布证据：先以 create_new/0600 写入固定临时名、用描述符设为 [`STAGED_MODE`]，
/// fsync 后再硬链接到最终名（目标已存在即失败，不覆盖既有证据）并同步目录。最终名因此只可能
/// 指向已落盘的内容，中断不会留下可被用作证据的半截 manifest。
pub(super) fn write_staged_manifest(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let name = path.file_name().context("暂存 manifest 缺少文件名")?;
    let directory = path.parent().context("暂存 manifest 缺少父目录")?;
    let mut temporary_name = name.to_os_string();
    temporary_name.push(".tmp");
    let temporary = path.with_file_name(temporary_name);
    // 固定临时名：上次中断只可能留下从未发布的残留，清理后重写即可。
    process::remove_stale(&temporary, "暂存 manifest 临时文件")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)
        .with_context(|| format!("创建暂存 manifest 临时文件失败：{}", temporary.display()))?;
    file.write_all(bytes).context("写入暂存 manifest 失败")?;
    file.set_permissions(fs::Permissions::from_mode(STAGED_MODE))
        .context("设置暂存 manifest 权限失败")?;
    file.sync_all().context("同步暂存 manifest 失败")?;
    let published = fs::hard_link(&temporary, path)
        .with_context(|| format!("发布暂存 manifest 失败：{}", path.display()));
    // 本次临时名无论发布成败都不再需要；清理失败不覆盖主结果。
    let _ = fs::remove_file(&temporary);
    published?;
    files::sync_directory(directory, "暂存目录")
}

/// 可重写的本机小证据：固定临时名先清理上次残留、再以 create_new/0600 新建，写满后设为
/// `mode` 并 fsync，最后改名覆盖目标并同步目录。只用于允许重复写入的上传证据，不用于只追加的
/// 发布证据；不截断既有路径，避免复用残留名时改写到别的链接指向的文件。
pub(super) fn replace_staged_file(
    target: &Path,
    temporary: &Path,
    bytes: &[u8],
    mode: u32,
) -> anyhow::Result<()> {
    process::remove_stale(temporary, "暂存证据临时文件")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(temporary)
        .with_context(|| format!("创建暂存证据临时文件失败：{}", temporary.display()))?;
    file.write_all(bytes).context("写入暂存证据失败")?;
    file.set_permissions(fs::Permissions::from_mode(mode)).context("设置暂存证据权限失败")?;
    file.sync_all().context("同步暂存证据失败")?;
    fs::rename(temporary, target)
        .with_context(|| format!("发布暂存证据失败：{}", target.display()))?;
    files::sync_directory(target.parent().context("暂存证据缺少父目录")?, "暂存目录")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt},
        path::PathBuf,
    };

    use super::{STAGED_MODE, replace_staged_file, temp_path, write_staged_manifest};

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("auction-wal-staging-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).expect("建立临时目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("设置权限");
            Self(root)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 发布证据不可覆盖，权限固定为组可读。
    #[test]
    fn staged_manifest_is_create_only_and_group_readable() {
        let root = TempRoot::new();
        let path = root.0.join("segment.zst.age.manifest.json");
        write_staged_manifest(&path, b"{}").expect("首次发布");
        assert_eq!(fs::metadata(&path).expect("元数据").mode() & 0o777, STAGED_MODE);
        assert!(write_staged_manifest(&path, b"[]").is_err(), "已存在的 manifest 不得覆盖");
        assert_eq!(fs::read(&path).expect("读取"), b"{}");
    }

    /// 可重写证据复用固定临时名（崩溃残留先清理再新建），按调用方给定的权限发布并可覆盖旧值。
    #[test]
    fn replaceable_evidence_reuses_temporary_name_and_mode() {
        let root = TempRoot::new();
        let target = root.0.join("segment.wal-upload.json");
        let temporary = temp_path(&root.0, "segment", ".wal-upload");
        assert_eq!(temporary, root.0.join("segment.wal-upload.tmp"));
        fs::write(&temporary, b"stale leftover that is longer").expect("残留临时文件");
        replace_staged_file(&target, &temporary, b"first", 0o644).expect("发布");
        assert_eq!(fs::read(&target).expect("读取"), b"first");
        assert_eq!(fs::metadata(&target).expect("元数据").mode() & 0o777, 0o644);
        assert!(!temporary.exists());
        replace_staged_file(&target, &temporary, b"second", 0o640).expect("覆盖");
        assert_eq!(fs::read(&target).expect("读取"), b"second");
        assert_eq!(fs::metadata(&target).expect("元数据").mode() & 0o777, 0o640);
    }
}
