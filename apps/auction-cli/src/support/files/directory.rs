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
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::disk::{DiskUsage, usage};
use super::trust::{Facts, Kind, first_untrusted};
use super::{effective_uid, ensure_private_dir, sync_parent};

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
        require_safe_path(path)?;
        let directory = Self::open_existing(path)?;
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
        require_safe_path(path)?;
        if create {
            match fs::DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => sync_parent(path)?,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("创建私有目录失败（父目录须已存在）：{}", path.display())
                    });
                }
            }
        }
        let directory = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(directory) => directory,
            Err(error) if !create && error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("打开私有目录失败：{}", path.display()));
            }
        };
        let directory = Self::pinned(path.to_owned(), directory)?;
        directory.verify_path()?;
        Ok(Some(directory))
    }

    /// 路径仍指向固定的目录：各级祖先仍然可信，且路径解析到的目录与描述符是同一个（设备号与
    /// inode）。祖先被改名、目录被移走或被替换时失败关闭。
    pub(crate) fn verify_path(&self) -> anyhow::Result<()> {
        require_safe_path(&self.path)?;
        let current = fs::symlink_metadata(&self.path).with_context(|| {
            format!("私有目录已不在原路径（祖先被改名或目录被移走）：{}", self.path.display())
        })?;
        let pinned = self.directory.metadata().context("读取私有目录元数据失败")?;
        if current.dev() != pinned.dev() || current.ino() != pinned.ino() {
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
        name_component(name)?;
        let child = Self::prepare(&self.path.join(name))?;
        if child.directory.metadata()?.dev() != self.directory.metadata()?.dev() {
            anyhow::bail!("私有子目录不得跨文件系统");
        }
        Ok(child)
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
    pub(crate) fn publish_file(&self, source: &str, target: &str) -> anyhow::Result<()> {
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

    pub(crate) fn remove(&self, name: &str, expected: FileIdentity) -> anyhow::Result<()> {
        if FileIdentity::of(&self.open(name)?.metadata()?) != expected {
            anyhow::bail!("私有文件身份或大小已变化，停止删除");
        }
        self.unlink(name)?;
        self.sync()
    }

    pub(crate) fn remove_child(&self, child: &Self) -> anyhow::Result<()> {
        let name =
            child.path.file_name().and_then(|name| name.to_str()).context("子目录名称无效")?;
        if child.path != self.path.join(name) || !child.names()?.is_empty() {
            anyhow::bail!("只允许删除已确认的空子目录");
        }
        let actual = self.open_at(name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?.metadata()?;
        let expected = child.directory.metadata()?;
        if actual.dev() != expected.dev() || actual.ino() != expected.ino() {
            anyhow::bail!("子目录身份变化");
        }
        let name = name_component(name)?;
        // SAFETY：父目录句柄有效，名称为已核对身份的单分量目录；内核拒绝非空目录和符号链接。
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
            != 0
        {
            return Err(std::io::Error::last_os_error()).context("删除空子目录失败");
        }
        self.sync()
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
        if !metadata.is_dir() || metadata.uid() != effective_uid() || metadata.mode() & 0o077 != 0 {
            anyhow::bail!("私有目录必须归当前账号所有且禁止 group/other 访问：{}", path.display());
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

fn name_component(name: &str) -> anyhow::Result<CString> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        anyhow::bail!("私有目录项必须是无控制字符的单个文件名");
    }
    CString::new(name).context("私有目录项名称无效")
}

fn require_private_file(file: &File) -> anyhow::Result<()> {
    let metadata = file.metadata().context("读取私有文件元数据失败")?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("私有文件必须是当前账号所有的单链接 owner-only 普通文件");
    }
    Ok(())
}

/// 路径须为不含 `.`/`..` 的绝对路径；从根开始已存在的每一级（含目录本身）都须是归 root 或
/// 当前账号、禁止 group/other 写入的真实目录（root 的 sticky 目录除外，判定见
/// [`first_untrusted`]），到第一个不存在的层级为止。
fn require_safe_path(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute()
        || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        anyhow::bail!("私有目录必须为不含 . 或 .. 的绝对路径：{}", path.display());
    }
    let mut prefix = PathBuf::new();
    let mut levels = Vec::new();
    for part in path.components() {
        prefix.push(part.as_os_str());
        let metadata = match fs::symlink_metadata(&prefix) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("读取私有目录祖先失败：{}", prefix.display()));
            }
        };
        let facts = Facts::of(&metadata);
        levels.push((prefix.clone(), facts));
        // 非目录（含符号链接）无法作为祖先，判定在这一级结束，不再穿过它读取下一级。
        if facts.kind != Kind::Directory {
            break;
        }
    }
    let chain = levels.iter().map(|(_, facts)| *facts).collect::<Vec<_>>();
    if let Some((index, problem)) = first_untrusted(&chain, effective_uid()) {
        let prefix = levels.get(index).map_or(path, |(prefix, _)| prefix.as_path());
        anyhow::bail!(
            "私有目录的祖先必须是 root 或当前账号所有、禁止他人写入的真实目录：{}（{problem}）",
            prefix.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use uuid::Uuid;

    use super::PrivateDirectory;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("auction-private-dir-{}", Uuid::new_v4()));
            fs::create_dir(&root).expect("建立临时目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("设置权限");
            Self(root)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 无覆盖发布不改写已有证据；可替换发布用于运行中状态。
    #[test]
    fn write_never_overwrites_without_replace() {
        let root = TempRoot::new();
        let directory = PrivateDirectory::prepare(&root.0.join("receipts")).expect("准备私有目录");
        directory.write("a.json", b"first", false).expect("首次发布");
        assert!(directory.write("a.json", b"second", false).is_err());
        assert_eq!(directory.read("a.json").expect("读取"), b"first");
        directory.write("a.json", b"third", true).expect("可替换发布");
        assert_eq!(directory.read("a.json").expect("读取"), b"third");
        let names = directory.names().expect("枚举");
        assert_eq!(names, vec!["a.json".to_owned()], "失败与成功路径都不残留临时名");
    }

    /// 祖先目录允许 group 写入时拒绝准备私有目录：他人可以替换其中的目录项。
    #[test]
    fn group_writable_ancestor_is_rejected() {
        let root = TempRoot::new();
        let shared = root.0.join("shared");
        fs::create_dir(&shared).expect("建立祖先目录");
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o770)).expect("设置权限");
        let error = PrivateDirectory::prepare(&shared.join("receipts"))
            .err()
            .expect("group 可写祖先必须拒绝");
        assert!(format!("{error:#}").contains("祖先"), "{error:#}");
        assert!(!shared.join("receipts").exists(), "拒绝前不创建目录");
    }

    /// `open_existing` 只核对目录本身：祖先对 group 可写不影响打开，目录本身对 group 开放或
    /// 是符号链接时拒绝。
    #[test]
    fn open_existing_checks_only_the_directory_itself() {
        let root = TempRoot::new();
        let shared = root.0.join("shared");
        fs::create_dir(&shared).expect("建立祖先目录");
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o770)).expect("设置权限");
        let receipts = shared.join("receipts");
        fs::create_dir(&receipts).expect("建立目录");
        fs::set_permissions(&receipts, fs::Permissions::from_mode(0o700)).expect("设置权限");
        let directory = PrivateDirectory::open_existing(&receipts).expect("祖先不在核对范围");
        directory.write("a.json", b"{}", false).expect("发布");

        fs::set_permissions(&receipts, fs::Permissions::from_mode(0o750)).expect("设置权限");
        assert!(PrivateDirectory::open_existing(&receipts).is_err(), "目录本身必须 owner-only");
        let link = shared.join("link");
        std::os::unix::fs::symlink(&receipts, &link).expect("创建符号链接");
        assert!(PrivateDirectory::open_existing(&link).is_err(), "拒绝符号链接");
        assert!(PrivateDirectory::open_existing(&shared.join("missing")).is_err());
    }

    /// 目录本身对 group 开放时拒绝，不放宽为可用的私有目录。
    #[test]
    fn non_private_directory_is_rejected() {
        let root = TempRoot::new();
        let open = root.0.join("open");
        fs::create_dir(&open).expect("建立目录");
        fs::set_permissions(&open, fs::Permissions::from_mode(0o750)).expect("设置权限");
        assert!(PrivateDirectory::prepare(&open).is_err());
        assert!(PrivateDirectory::prepare(&root.0.join("..").join("x")).is_err(), "拒绝 ..");
    }
}
