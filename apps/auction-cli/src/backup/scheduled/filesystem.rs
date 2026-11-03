//! 定时备份私有目录与目录句柄操作；拒绝链接、特殊文件和越界名称，删除前复核文件身份。

use std::{
    ffi::CString,
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::support::files;

pub(super) const MAX_RECORD_BYTES: u64 = 1024 * 1024;

/// 只在本机回执中用于防替换；不是内容校验或可移植恢复契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
}

impl FileIdentity {
    pub(super) fn of(metadata: &Metadata) -> Self {
        Self { device: metadata.dev(), inode: metadata.ino(), bytes: metadata.len() }
    }
}

/// 已固定的 owner-only 目录；子项读写和删除都相对于同一个目录描述符。
pub(super) struct PrivateDirectory {
    pub path: PathBuf,
    pub(super) directory: File,
}

impl PrivateDirectory {
    pub(super) fn prepare(path: &Path) -> anyhow::Result<Self> {
        require_safe_path(path)?;
        files::ensure_private_dir(path)?;
        require_safe_path(path)?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .context("打开定时备份私有目录失败")?;
        directory.sync_all().context("同步备份私有目录失败")?;
        files::sync_parent(path)?;
        Ok(Self { path: path.to_owned(), directory })
    }

    /// 子目录必须留在同一文件系统，保证密文占用纳入输出目录的水位门禁。
    pub(super) fn child(&self, name: &str) -> anyhow::Result<Self> {
        name_component(name)?;
        let child = Self::prepare(&self.path.join(name))?;
        if child.directory.metadata()?.dev() != self.directory.metadata()?.dev() {
            anyhow::bail!("备份私有子目录不得跨文件系统");
        }
        Ok(child)
    }

    pub(super) fn names(&self) -> anyhow::Result<Vec<String>> {
        // 原生程序限定 Linux；经描述符枚举，祖先目录改名不会把删除目标导向另一棵树。
        let entries = fs::read_dir(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
            .context("枚举定时备份私有目录失败")?;
        entries
            .map(|entry| {
                let entry = entry.context("读取定时备份目录项失败")?;
                Ok(entry.file_name().into_string().unwrap_or_else(|_| "<non_utf8>".to_owned()))
            })
            .collect()
    }

    pub(super) fn open(&self, name: &str) -> anyhow::Result<File> {
        let file = self.open_at(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        require_private_file(&file)?;
        Ok(file)
    }

    pub(super) fn read(&self, name: &str) -> anyhow::Result<Vec<u8>> {
        let file = self.open(name)?;
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes).context("读取备份运行记录失败")?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "备份运行记录超过大小上限",
            )
            .into());
        }
        Ok(bytes)
    }

    /// 返回 None 表示另一个活跃进程仍持有锁；不等待，也不把活跃运行标成 interrupted。
    pub(super) fn try_lock(&self, name: &str) -> anyhow::Result<Option<File>> {
        let file = self.open_at(name, libc::O_RDWR | libc::O_CREAT | libc::O_NONBLOCK, 0o600)?;
        require_private_file(&file)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => Err(error).context("取得定时备份锁失败"),
        }
    }

    /// 回执无覆盖发布；只有运行中状态允许原子替换，二者均先同步文件再同步目录。
    pub(super) fn write(&self, name: &str, bytes: &[u8], replace: bool) -> anyhow::Result<()> {
        let target = name_component(name)?;
        let temporary_name = format!(".{}.partial", Uuid::new_v4());
        let temporary = name_component(&temporary_name)?;
        let mut temporary_created = false;
        let result = (|| {
            let mut file = self.open_at(
                &temporary_name,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )?;
            temporary_created = true;
            file.write_all(bytes).context("写入备份运行记录失败")?;
            file.sync_all().context("同步备份运行记录失败")?;
            let directory = self.directory.as_raw_fd();
            // SAFETY：目录描述符在 self 生命周期内有效，名称是无 NUL 的单路径分量。
            // 原子改名保持单链接；即使发布后断电，也不会残留双硬链接而阻止下一次收口。
            let status = unsafe {
                libc::renameat2(
                    directory,
                    temporary.as_ptr(),
                    directory,
                    target.as_ptr(),
                    if replace { 0 } else { libc::RENAME_NOREPLACE },
                )
            };
            if status != 0 {
                return Err(std::io::Error::last_os_error()).context("发布备份运行记录失败");
            }
            Ok(())
        })();
        // 成功改名已移走临时名；失败路径只回收本次 create_new 的临时名。
        if temporary_created {
            let _ = self.unlink(&temporary_name);
        }
        result?;
        self.sync()
    }

    /// 私有普通文件先同步 0600 权限，再无覆盖发布；也用于原样保留无效记录。
    pub(super) fn publish_file(&self, source: &str, target: &str) -> anyhow::Result<()> {
        let file = self.open(source)?;
        file.set_permissions(fs::Permissions::from_mode(0o600)).context("设置私有文件权限失败")?;
        file.sync_all().context("同步私有文件失败")?;
        let source = name_component(source)?;
        let target = name_component(target)?;
        let directory = self.directory.as_raw_fd();
        // SAFETY：目录描述符有效，两个名称均为受控单分量；禁止覆盖已有保留文件。
        if unsafe {
            libc::renameat2(
                directory,
                source.as_ptr(),
                directory,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error()).context("无覆盖发布私有文件失败");
        }
        self.sync()
    }

    pub(super) fn remove(&self, name: &str, expected: FileIdentity) -> anyhow::Result<()> {
        if FileIdentity::of(&self.open(name)?.metadata()?) != expected {
            anyhow::bail!("备份文件身份或大小已变化，停止删除");
        }
        self.unlink(name)?;
        self.sync()
    }

    pub(super) fn remove_child(&self, child: &Self) -> anyhow::Result<()> {
        let name =
            child.path.file_name().and_then(|name| name.to_str()).context("子目录名称无效")?;
        if child.path != self.path.join(name) || !child.names()?.is_empty() {
            anyhow::bail!("只允许删除已确认的空暂存子目录");
        }
        let actual = self.open_at(name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?.metadata()?;
        let expected = child.directory.metadata()?;
        if actual.dev() != expected.dev() || actual.ino() != expected.ino() {
            anyhow::bail!("暂存子目录身份变化");
        }
        let name = name_component(name)?;
        // SAFETY：父目录句柄有效，名称为已核对身份的单分量目录；内核拒绝非空目录和符号链接。
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
            != 0
        {
            return Err(std::io::Error::last_os_error()).context("删除暂存空目录失败");
        }
        self.sync()
    }

    pub(super) fn sync(&self) -> anyhow::Result<()> {
        self.directory.sync_all().context("同步备份私有目录失败")
    }

    fn open_at(&self, name: &str, flags: i32, mode: libc::mode_t) -> anyhow::Result<File> {
        let name = name_component(name)?;
        // SAFETY：目录句柄有效，名称已限制为单分量；成功返回的描述符只交给 File 一次。
        let descriptor = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error()).context("打开备份私有目录项失败");
        }
        // SAFETY：openat 成功，descriptor 由当前调用独占拥有。
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    fn unlink(&self, name: &str) -> anyhow::Result<()> {
        let name = name_component(name)?;
        // SAFETY：相对有效目录句柄删除单个普通目录项，不跟随符号链接。
        let status = unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) };
        if status != 0 {
            return Err(std::io::Error::last_os_error()).context("删除备份私有目录项失败");
        }
        Ok(())
    }
}

fn name_component(name: &str) -> anyhow::Result<CString> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        anyhow::bail!("备份目录项必须是无控制字符的单个文件名");
    }
    CString::new(name).context("备份目录项名称无效")
}

fn require_private_file(file: &File) -> anyhow::Result<()> {
    let metadata = file.metadata().context("读取备份文件元数据失败")?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != files::effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("备份文件必须是当前账号所有的单链接 owner-only 普通文件");
    }
    Ok(())
}

fn require_safe_path(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute()
        || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        anyhow::bail!("备份目录必须为不含 . 或 .. 的绝对路径");
    }
    let mut prefix = PathBuf::new();
    for part in path.components() {
        prefix.push(part.as_os_str());
        let metadata = match fs::symlink_metadata(&prefix) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error).context("读取备份目录祖先失败"),
        };
        // root 拥有的 sticky 临时根允许私有子目录；其余祖先禁止他人写入。
        let sticky_root = metadata.uid() == 0 && metadata.mode() & libc::S_ISVTX != 0;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || (metadata.uid() != 0 && metadata.uid() != files::effective_uid())
            || (metadata.mode() & 0o022 != 0 && !sticky_root)
        {
            anyhow::bail!("备份目录祖先必须是真实可信目录");
        }
    }
    Ok(())
}
