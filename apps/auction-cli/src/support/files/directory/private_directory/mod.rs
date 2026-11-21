//! 以目录描述符固定的 owner-only 私有目录：子项读写、无覆盖或可替换发布、按身份删除与子目录
//! 派生都相对同一个描述符完成，拒绝链接、特殊文件与越界名称。定时备份状态与部署状态目录（锁、
//! 回执、回滚记录与保留程序）共用这一份实现。
//!
//! 不变量：发布先写满 `.{uuid}.partial` 临时文件（O_EXCL、0600）并 fsync，再以 `renameat2`
//! 改名（回执与终态用 `RENAME_NOREPLACE`，从不覆盖已有证据），最后同步目录；失败只回收本次
//! 创建的临时名。经可信入口固定的目录，此后的子目录与文件都相对描述符打开，祖先被改名也不会
//! 把读写导向另一棵树；`verify_path` 用于在不可逆步骤前确认路径仍指向同一个目录。

use std::{
    ffi::CString,
    fs::{self, File, Metadata, OpenOptions},
    io::{ErrorKind, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::disk::{DiskUsage, usage};
use super::super::{effective_uid, ensure_private_dir, sync_parent};
use super::path_helpers::{
    name_component, open_trusted_ancestors, require_private_file, require_safe_path,
};
mod entry_lock;
mod removal;
use self::entry_lock::lock_directory_entries;

/// [`PrivateDirectory::read`] 的读取上限：目录内只放小型记录与回执。
pub(crate) const MAX_READ_BYTES: u64 = 1024 * 1024;

/// 只在本机回执中用于防替换；不是内容校验或可移植恢复契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
}

impl FileIdentity {
    pub(crate) fn of(metadata: &Metadata) -> Self {
        Self { device: metadata.dev(), inode: metadata.ino(), bytes: metadata.len() }
    }
}

/// 已固定的 owner-only 目录；子项读写和删除都相对于同一个目录描述符。
pub(crate) struct PrivateDirectory {
    pub path: PathBuf,
    directory: File,
}

impl PrivateDirectory {
    /// 创建或复核私有目录：路径为不含 `.`/`..` 的绝对路径，已存在的祖先都是 root 或当前账号
    /// 所有且禁止他人写入（root 的 sticky 临时根除外）；目录本身按 [`Self::open_existing`]
    /// 核对。定时备份以服务账号运行在自有目录树下，使用这一更严格的入口。
    pub(crate) fn prepare(path: &Path) -> anyhow::Result<Self> {
        require_safe_path(path)?;
        ensure_private_dir(path)?;
        let directory =
            Self::open_trusted(path, false)?.context("创建后无法通过安全目录遍历固定私有目录")?;
        directory.sync()?;
        sync_parent(path)?;
        Ok(directory)
    }

    /// 打开已存在的私有目录，只在打开的描述符上核对目录本身：非符号链接、归当前账号所有、
    /// 无 group/other 权限；不检查祖先，祖先由调用方核对（见 [`Self::prepare`] 与
    /// [`Self::open_trusted`]）。
    pub(crate) fn open_existing(path: &Path) -> anyhow::Result<Self> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .with_context(|| format!("打开私有目录失败：{}", path.display()))?;
        Self::pinned(path.to_owned(), directory)
    }

    /// 打开以描述符固定的可信私有目录（部署状态目录的入口）：路径上已存在的每一级都必须归 root
    /// 或当前账号且禁止 group/other 写入（root 的 sticky 目录除外），目录本身归当前账号且禁止
    /// group/other 访问，打开后路径仍须指向同一个目录。`create` 时只以 0700 创建最后一级，父目录
    /// 必须已存在：不替调用方建立祖先，避免把本应 0755 的公共父目录建成 0700。`create` 为假且
    /// 目录不存在时返回 None。
    pub(crate) fn open_trusted(path: &Path, create: bool) -> anyhow::Result<Option<Self>> {
        let parent_path = path.parent().context("私有目录路径缺少父目录")?;
        let parent = match open_trusted_ancestors(parent_path) {
            Ok(parent) => parent,
            Err(error)
                if !create
                    && error.chain().any(|cause| {
                        cause
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|io| io.kind() == ErrorKind::NotFound)
                    }) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let name = path.file_name().context("私有目录路径缺少名称")?;
        let name = CString::new(name.as_bytes()).context("私有目录名称包含 NUL")?;
        let _entry_lock = lock_directory_entries(&parent)?;
        if create {
            // SAFETY：parent fd 已逐级固定并验证，name 是单个普通路径分量。
            if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } == 0 {
                parent.sync_all().context("同步私有目录父目录失败")?;
            } else {
                let error = std::io::Error::last_os_error();
                if error.kind() != ErrorKind::AlreadyExists {
                    return Err(error).with_context(|| {
                        format!("创建私有目录失败（父目录须已存在）：{}", path.display())
                    });
                }
            }
        }
        // SAFETY：parent fd 及单分量 name 已验证；禁止跟随替换成符号链接的最终目录项。
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if !create && error.kind() == ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).with_context(|| format!("打开私有目录失败：{}", path.display()));
        }
        // SAFETY：openat 成功，descriptor 由当前调用独占拥有。
        let directory = unsafe { File::from_raw_fd(descriptor) };
        let directory = Self::pinned(path.to_owned(), directory)?;
        directory.verify_path()?;
        Ok(Some(directory))
    }

    /// 路径仍指向固定的目录：各级祖先仍然可信，且路径解析到的目录与描述符是同一个（设备号与
    /// inode）。祖先被改名、目录被移走或被替换时失败关闭。
    pub(crate) fn verify_path(&self) -> anyhow::Result<()> {
        require_safe_path(&self.path)?;
        let parent_path = self.path.parent().context("私有目录路径缺少父目录")?;
        let parent = open_trusted_ancestors(parent_path)?;
        let name = self.path.file_name().context("私有目录路径缺少名称")?;
        let name = CString::new(name.as_bytes()).context("私有目录名称包含 NUL")?;
        // SAFETY: parent is pinned and name is one NUL-free component; the returned descriptor is owned below.
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("私有目录已不在原路径：{}", self.path.display()));
        }
        // SAFETY: openat returned a new descriptor owned by this call.
        let current = unsafe { File::from_raw_fd(descriptor) };
        let current_metadata = current.metadata().context("读取当前私有目录元数据失败")?;
        let pinned = self.directory.metadata().context("读取私有目录元数据失败")?;
        let mut entry = std::mem::MaybeUninit::<libc::stat>::zeroed();
        // SAFETY: parent and name are valid; entry points to writable stat storage.
        if unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                entry.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("复核私有目录路径失败：{}", self.path.display()));
        }
        // SAFETY: fstatat initialized entry after returning success.
        let entry = unsafe { entry.assume_init() };
        if current_metadata.dev() != pinned.dev()
            || current_metadata.ino() != pinned.ino()
            || entry.st_dev != pinned.dev()
            || entry.st_ino != pinned.ino()
            || entry.st_mode & libc::S_IFMT != libc::S_IFDIR
        {
            anyhow::bail!(
                "私有目录路径已指向另一个目录（祖先被改名或目录被替换）：{}",
                self.path.display()
            );
        }
        Ok(())
    }

    /// 相对本目录的描述符打开子目录，不重新解析路径；`create` 时缺失则以 0700 创建。子目录同样
    /// 必须归当前账号、禁止 group/other 访问，且与本目录在同一文件系统。`create` 为假且子目录
    /// 不存在时返回 None。
    pub(crate) fn open_child(&self, name: &str, create: bool) -> anyhow::Result<Option<Self>> {
        let _entry_lock = create.then(|| lock_directory_entries(&self.directory)).transpose()?;
        if create {
            self.make_child(name)?;
        }
        let directory = match self.openat_raw(name, libc::O_RDONLY | libc::O_DIRECTORY, 0) {
            Ok(directory) => directory,
            Err(error) if !create && error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("打开私有子目录失败：{}", self.path.join(name).display())
                });
            }
        };
        self.adopt_child(name, directory).map(Some)
    }

    /// 新建子目录：已存在即失败，用于只追加、不覆盖的记录目录。
    pub(crate) fn create_child(&self, name: &str) -> anyhow::Result<Self> {
        let _entry_lock = lock_directory_entries(&self.directory)?;
        if !self.make_child(name)? {
            anyhow::bail!("私有子目录已存在，拒绝覆盖：{}", self.path.join(name).display());
        }
        let directory = self
            .openat_raw(name, libc::O_RDONLY | libc::O_DIRECTORY, 0)
            .with_context(|| format!("打开私有子目录失败：{}", self.path.join(name).display()))?;
        self.adopt_child(name, directory)
    }

    /// 子目录必须留在同一文件系统，保证其中的文件纳入父目录的水位门禁。
    pub(crate) fn child(&self, name: &str) -> anyhow::Result<Self> {
        self.open_child(name, true)?.context("创建后无法打开私有子目录")
    }

    /// 目录所在文件系统的容量；读取的是已固定的描述符，不重新解析路径。
    pub(crate) fn usage(&self) -> anyhow::Result<DiskUsage> {
        usage(&self.directory)
    }

    pub(crate) fn names(&self) -> anyhow::Result<Vec<String>> {
        // 原生程序限定 Linux；经描述符枚举，祖先目录改名不会把删除目标导向另一棵树。
        let entries = fs::read_dir(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
            .context("枚举私有目录失败")?;
        entries
            .map(|entry| {
                let entry = entry.context("读取私有目录项失败")?;
                Ok(entry.file_name().into_string().unwrap_or_else(|_| "<non_utf8>".to_owned()))
            })
            .collect()
    }

    pub(crate) fn open(&self, name: &str) -> anyhow::Result<File> {
        let file = self.open_at(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        require_private_file(&file)?;
        Ok(file)
    }

    /// 打开当前账号所有的单链接普通文件：禁止 group/other 写入，但允许他人读取与执行（如保留的
    /// 0755 程序副本）；元数据核对与后续读取使用同一个描述符。
    pub(crate) fn open_owned(&self, name: &str) -> anyhow::Result<File> {
        let file = self.open_at(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let metadata = file.metadata().context("读取文件元数据失败")?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o022 != 0
        {
            anyhow::bail!(
                "必须是当前账号所有、单链接、禁止 group/other 写入的普通文件：{}",
                self.path.join(name).display()
            );
        }
        Ok(file)
    }

    /// 新建只写文件（`O_EXCL`，不跟随符号链接），权限由调用方给出；已存在即失败。
    pub(crate) fn create_file(&self, name: &str, mode: libc::mode_t) -> anyhow::Result<File> {
        let _entry_lock = lock_directory_entries(&self.directory)?;
        self.open_at(name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, mode)
            .with_context(|| format!("新建文件失败：{}", self.path.join(name).display()))
    }

    /// 目录项是否存在；不跟随符号链接，链接本身也算存在。
    pub(crate) fn contains(&self, name: &str) -> anyhow::Result<bool> {
        match self.openat_raw(name, libc::O_PATH, 0) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error)
                .with_context(|| format!("读取私有目录项失败：{}", self.path.join(name).display())),
        }
    }

    pub(crate) fn read(&self, name: &str) -> anyhow::Result<Vec<u8>> {
        let file = self.open(name)?;
        let mut bytes = Vec::new();
        file.take(MAX_READ_BYTES + 1).read_to_end(&mut bytes).context("读取私有文件失败")?;
        if bytes.len() as u64 > MAX_READ_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "私有文件超过大小上限",
            )
            .into());
        }
        Ok(bytes)
    }

    /// 返回 None 表示另一个活跃进程仍持有锁；不等待，也不把活跃运行标成 interrupted。
    pub(crate) fn try_lock(&self, name: &str) -> anyhow::Result<Option<File>> {
        let file = self.open_at(name, libc::O_RDWR | libc::O_CREAT | libc::O_NONBLOCK, 0o600)?;
        require_private_file(&file)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => Err(error).context("取得私有目录锁失败"),
        }
    }

    /// `replace` 为假时无覆盖发布（回执、终态与完成事实）；只有运行中状态允许原子替换。
    /// 两种都先同步文件再同步目录。
    pub(crate) fn write(&self, name: &str, bytes: &[u8], replace: bool) -> anyhow::Result<()> {
        let _entry_lock = lock_directory_entries(&self.directory)?;
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
            file.write_all(bytes).context("写入私有文件失败")?;
            file.sync_all().context("同步私有文件失败")?;
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
                return Err(std::io::Error::last_os_error()).context("发布私有文件失败");
            }
            temporary_created = false;
            Ok(())
        })();
        // 成功改名已移走临时名；失败路径只回收本次 create_new 的临时名。
        let cleanup = if temporary_created {
            match self.unlink(&temporary_name) {
                Err(error)
                    if error.chain().any(|cause| {
                        cause
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|io| io.kind() == ErrorKind::NotFound)
                    }) =>
                {
                    Ok(())
                }
                result => result,
            }
        } else {
            Ok(())
        };
        match result {
            Err(primary) => match cleanup {
                Ok(()) => Err(primary),
                Err(cleanup) => Err(primary.context(format!("清理私有临时文件失败：{cleanup:#}"))),
            },
            Ok(()) => self.sync().context("私有文件已原子发布，但同步目录失败；请核对目标状态"),
        }
    }

    /// 私有普通文件先同步 0600 权限，再无覆盖发布；也用于原样保留无效记录。
    pub(crate) fn publish_file(&self, source: &str, target: &str) -> anyhow::Result<()> {
        let _entry_lock = lock_directory_entries(&self.directory)?;
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
            .context("私有文件已无覆盖发布，但目录同步失败；发布已提交，持久性未知，请核对目标状态")
    }

    pub(crate) fn sync(&self) -> anyhow::Result<()> {
        self.directory.sync_all().context("同步私有目录失败")
    }

    fn open_at(&self, name: &str, flags: i32, mode: libc::mode_t) -> anyhow::Result<File> {
        self.openat_raw(name, flags, mode).context("打开私有目录项失败")
    }

    /// 保留原始 I/O 错误，供调用方区分“不存在”与其他失败。
    fn openat_raw(&self, name: &str, flags: i32, mode: libc::mode_t) -> std::io::Result<File> {
        let name = name_component(name)
            .map_err(|error| std::io::Error::new(ErrorKind::InvalidInput, format!("{error:#}")))?;
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
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY：openat 成功，descriptor 由当前调用独占拥有。
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    /// 以 0700 创建子目录；返回 false 表示已存在。创建后同步本目录，使新目录项在崩溃后可见。
    fn make_child(&self, name: &str) -> anyhow::Result<bool> {
        let component = name_component(name)?;
        // SAFETY：目录描述符有效，名称是单分量；mkdirat 不跟随最后一级的符号链接。
        if unsafe { libc::mkdirat(self.directory.as_raw_fd(), component.as_ptr(), 0o700) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == ErrorKind::AlreadyExists {
                return Ok(false);
            }
            return Err(error).with_context(|| {
                format!("创建私有子目录失败：{}", self.path.join(name).display())
            });
        }
        self.sync()?;
        Ok(true)
    }

    /// 子目录描述符的核对：归当前账号、禁止 group/other 访问、与本目录在同一文件系统。
    fn adopt_child(&self, name: &str, directory: File) -> anyhow::Result<Self> {
        let child = Self::pinned(self.path.join(name), directory)?;
        if child.directory.metadata()?.dev() != self.directory.metadata()?.dev() {
            anyhow::bail!("私有子目录不得跨文件系统：{}", child.path.display());
        }
        Ok(child)
    }

    fn pinned(path: PathBuf, directory: File) -> anyhow::Result<Self> {
        let metadata = directory.metadata().context("读取私有目录元数据失败")?;
        if !metadata.is_dir()
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o077 != 0
            || metadata.mode() & 0o700 != 0o700
        {
            anyhow::bail!(
                "私有目录必须归当前账号所有、owner 位为 0700 且禁止 group/other 访问：{}",
                path.display()
            );
        }
        Ok(Self { path, directory })
    }

    fn unlink(&self, name: &str) -> anyhow::Result<()> {
        let name = name_component(name)?;
        // SAFETY：相对有效目录句柄删除单个普通目录项，不跟随符号链接。
        let status = unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) };
        if status != 0 {
            return Err(std::io::Error::last_os_error()).context("删除私有目录项失败");
        }
        Ok(())
    }
}
