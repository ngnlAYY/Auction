//! 安装目录的信任边界与以描述符固定的安装操作。
//!
//! root 与 postgres 都会执行安装目录中的 auction-cli（部署编排、`archive_command`），因此 root
//! 运行时安装目录、它的每一级祖先与其中的程序都必须归 root 所有、不是符号链接且禁止 group/other
//! 写入，新程序一律 root:root 0755。非 root 运行只用于不跨账号的隔离环境：可信属主为当前账号，
//! 祖先与既有程序也可以归 root。门禁只读、不修补：不安全的既有安装不会被自动 chown，须由维护者
//! 用已验证的 Release 重建信任。
//!
//! 安装、换回与复核都相对同一个以 `O_DIRECTORY | O_NOFOLLOW` 打开的目录描述符完成：临时文件以
//! `O_EXCL | O_NOFOLLOW` 新建，`renameat` 原子替换，替换后重新打开目标复核属主、权限与摘要。

use std::{
    ffi::CString,
    fs::{self, File, Metadata, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

use anyhow::Context;
use uuid::Uuid;

use super::super::artifact::REQUIRED_BINARIES;
use crate::support::digest::sha256_reader;
use crate::support::files;

/// 安装目录中程序的唯一权限。
pub(crate) const PROGRAM_MODE: u32 = 0o755;
/// 门禁拒绝时的处置：不自动修补，由维护者从已验证的 Release 重建信任。
const REBUILD_TRUST: &str = "不会自动修补属主或权限：请按 ops/deploy/RECOVERY.md 的“重建安装目录信任”\
                             用已验证的 Release 重装为 root:root 0755 后重试";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Directory,
    File,
    Other,
}

/// 判定所需的元数据事实；与真实文件解耦，非 root 的测试也能覆盖 root 口径。
#[derive(Debug, Clone, Copy)]
struct Facts {
    kind: Kind,
    uid: u32,
    mode: u32,
}

impl Facts {
    fn of(metadata: &Metadata) -> Self {
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            Kind::Other
        } else if file_type.is_dir() {
            Kind::Directory
        } else if file_type.is_file() {
            Kind::File
        } else {
            Kind::Other
        };
        Self { kind, uid: metadata.uid(), mode: metadata.mode() }
    }
}

/// 祖先目录：真实目录、归 root 或当前账号、禁止 group/other 写入。root 拥有的 sticky 目录除外：
/// 他人不能改名或删除其中不属于自己的目录项。
fn ancestor_problem(facts: Facts, euid: u32) -> Option<&'static str> {
    if facts.kind != Kind::Directory {
        return Some("不是真实目录");
    }
    if facts.uid != 0 && facts.uid != euid {
        return Some("属主不是 root 或当前账号");
    }
    let sticky_root = facts.uid == 0 && facts.mode & libc::S_ISVTX != 0;
    if facts.mode & 0o022 != 0 && !sticky_root {
        return Some("允许 group/other 写入");
    }
    None
}

/// 安装目录本身：真实目录、归当前账号（root 运行即 root）、禁止 group/other 写入。
fn directory_problem(facts: Facts, euid: u32) -> Option<&'static str> {
    if facts.kind != Kind::Directory {
        return Some("不是真实目录");
    }
    if facts.uid != euid {
        return Some(if euid == 0 { "属主不是 root" } else { "属主不是当前账号" });
    }
    if facts.mode & 0o022 != 0 {
        return Some("允许 group/other 写入");
    }
    None
}

/// 既有程序：普通文件、归可信属主（root 运行时只能是 root；否则 root 或当前账号）、禁止
/// group/other 写入。归服务账号所有的程序在这里拒绝，被服务账号改写过的 CLI 不能交给 root 执行。
fn program_problem(facts: Facts, euid: u32) -> Option<&'static str> {
    if facts.kind != Kind::File {
        return Some("不是普通文件");
    }
    if facts.uid != 0 && facts.uid != euid {
        return Some(if euid == 0 {
            "属主不是 root"
        } else {
            "属主不是 root 或当前账号"
        });
    }
    if facts.mode & 0o022 != 0 {
        return Some("允许 group/other 写入");
    }
    None
}

/// 停机前门禁（deploy 与 rollback，dry-run 同样执行）：只读核对安装目录、它的每一级祖先与其中
/// 四个程序；不创建、不修改任何文件。
pub(crate) fn require_install_dir(path: &Path) -> anyhow::Result<()> {
    require_trusted_directory(path)?;
    let euid = files::effective_uid();
    for name in REQUIRED_BINARIES {
        let program = path.join(name);
        let metadata = fs::symlink_metadata(&program)
            .with_context(|| format!("读取当前安装程序失败：{}", program.display()))?;
        if let Some(problem) = program_problem(Facts::of(&metadata), euid) {
            anyhow::bail!("安装目录中的 {name} 不可信（{problem}）；{REBUILD_TRUST}");
        }
    }
    Ok(())
}

/// 安装目录与每一级祖先；能替换其中任一目录项的账号就能替换 root 执行的程序。
fn require_trusted_directory(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute()
        || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        anyhow::bail!("安装目录必须是不含 . 或 .. 的绝对路径：{}", path.display());
    }
    let euid = files::effective_uid();
    let components = path.components().collect::<Vec<_>>();
    let mut prefix = PathBuf::new();
    for (index, part) in components.iter().enumerate() {
        prefix.push(part.as_os_str());
        let metadata = fs::symlink_metadata(&prefix)
            .with_context(|| format!("读取安装目录失败：{}", prefix.display()))?;
        let facts = Facts::of(&metadata);
        if index + 1 < components.len() {
            if let Some(problem) = ancestor_problem(facts, euid) {
                anyhow::bail!(
                    "安装目录的祖先 {} 不可信（{problem}）；{REBUILD_TRUST}",
                    prefix.display()
                );
            }
        } else if let Some(problem) = directory_problem(facts, euid) {
            anyhow::bail!("安装目录 {} 不可信（{problem}）；{REBUILD_TRUST}", prefix.display());
        }
    }
    Ok(())
}

/// 已安装程序的当前事实；读取与元数据来自同一个不跟随符号链接的描述符。
pub(crate) struct Inspection {
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    /// 属主、属组与权限是否符合安装后的要求（root:root 或当前账号，0755）。
    pub(crate) sealed: bool,
}

/// 以目录描述符固定的可信安装目录；子项的新建、改名、读取与删除都相对同一个描述符。
pub(crate) struct PinnedInstallDir {
    path: PathBuf,
    directory: File,
    euid: u32,
}

impl PinnedInstallDir {
    /// 安装与换回前重新核对安装目录与祖先，再在打开的描述符上复核目录本身。程序本身的信任在
    /// 停机前由 [`require_install_dir`] 核对；这里不重查，换回时目标可能已被部分替换。
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
        require_trusted_directory(path)?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .with_context(|| format!("打开安装目录失败：{}", path.display()))?;
        let euid = files::effective_uid();
        let metadata = directory.metadata().context("读取安装目录元数据失败")?;
        if let Some(problem) = directory_problem(Facts::of(&metadata), euid) {
            anyhow::bail!("安装目录 {} 不可信（{problem}）；{REBUILD_TRUST}", path.display());
        }
        Ok(Self { path: path.to_owned(), directory, euid })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// 新建同目录临时程序（`O_EXCL`、0755）；返回临时名与可写描述符。
    pub(crate) fn create_temporary(&self, name: &str) -> anyhow::Result<(String, File)> {
        let temporary = format!(".{name}.{}.new", Uuid::new_v4());
        let file = self
            .open_at(&temporary, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, PROGRAM_MODE)
            .with_context(|| format!("创建安装临时文件失败：{}", self.path.join(name).display()))?;
        Ok((temporary, file))
    }

    /// 把写完的临时程序定为可信属主与 0755 并落盘：root 运行时显式设为 root:root，不受目录
    /// setgid 或调用者属组影响；非 root 时保持当前账号。
    pub(crate) fn seal(&self, file: &File) -> anyhow::Result<()> {
        if self.euid == 0 {
            std::os::unix::fs::fchown(file, Some(0), Some(0)).context("设置程序属主失败")?;
        }
        file.set_permissions(fs::Permissions::from_mode(PROGRAM_MODE))
            .context("设置程序权限失败")?;
        file.sync_all().context("同步安装临时文件失败")
    }

    /// 同目录原子改名；`to` 已存在时被替换。
    pub(crate) fn rename(&self, from: &str, to: &str) -> anyhow::Result<()> {
        let from = component(from)?;
        let to = component(to)?;
        let directory = self.directory.as_raw_fd();
        // SAFETY：目录描述符在 self 生命周期内有效，两个名称都是无 NUL 的单路径分量。
        if unsafe { libc::renameat(directory, from.as_ptr(), directory, to.as_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error()).context("改名失败");
        }
        Ok(())
    }

    /// 删除本次创建的临时名；不跟随符号链接。
    pub(crate) fn remove(&self, name: &str) -> anyhow::Result<()> {
        let name = component(name)?;
        // SAFETY：目录描述符有效，名称是单路径分量；flags 为 0 只删除非目录项。
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error()).context("删除安装临时文件失败");
        }
        Ok(())
    }

    pub(crate) fn sync(&self) -> anyhow::Result<()> {
        self.directory
            .sync_all()
            .with_context(|| format!("同步安装目录失败：{}", self.path.display()))
    }

    /// 重新打开目录中的程序并读取事实；不是普通文件时失败。
    pub(crate) fn inspect(&self, name: &str) -> anyhow::Result<Inspection> {
        let file = self
            .open_at(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)
            .with_context(|| format!("打开安装目录中的程序失败：{name}"))?;
        let metadata = file.metadata().context("读取程序元数据失败")?;
        if !metadata.is_file() {
            anyhow::bail!("安装目录中的 {name} 不是普通文件");
        }
        let sealed = metadata.uid() == self.euid
            && (self.euid != 0 || metadata.gid() == 0)
            && metadata.mode() & 0o7777 == PROGRAM_MODE;
        let (sha256, bytes) =
            sha256_reader(&file).with_context(|| format!("读取安装目录中的程序失败：{name}"))?;
        Ok(Inspection { sha256, bytes, sealed })
    }

    fn open_at(&self, name: &str, flags: i32, mode: u32) -> anyhow::Result<File> {
        let name = component(name)?;
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
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY：openat 成功，descriptor 由当前调用独占拥有。
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

fn component(name: &str) -> anyhow::Result<CString> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        anyhow::bail!("安装目录项必须是单个文件名：{name}");
    }
    CString::new(name).context("安装目录项名称无效")
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use super::{
        Facts, Kind, PinnedInstallDir, ancestor_problem, directory_problem, program_problem,
        require_install_dir,
    };
    use crate::deploy::artifact::REQUIRED_BINARIES;

    const SERVICE_UID: u32 = 990;

    fn facts(kind: Kind, uid: u32, mode: u32) -> Facts {
        Facts { kind, uid, mode }
    }

    /// root 运行时只信任 root：服务账号所有的程序或目录、group/other 可写与非普通文件都拒绝；
    /// root 拥有的 sticky 祖先（如 /tmp）可以，他人不能替换其中不属于自己的目录项。
    #[test]
    fn root_trusts_only_root_owned_entries() {
        assert_eq!(program_problem(facts(Kind::File, 0, 0o100_755), 0), None);
        assert!(program_problem(facts(Kind::File, SERVICE_UID, 0o100_755), 0).is_some());
        assert!(program_problem(facts(Kind::File, 0, 0o100_775), 0).is_some());
        assert!(program_problem(facts(Kind::File, 0, 0o100_757), 0).is_some());
        assert!(program_problem(facts(Kind::Other, 0, 0o120_777), 0).is_some(), "符号链接");
        assert_eq!(directory_problem(facts(Kind::Directory, 0, 0o40_755), 0), None);
        assert!(directory_problem(facts(Kind::Directory, SERVICE_UID, 0o40_700), 0).is_some());
        assert!(directory_problem(facts(Kind::Directory, 0, 0o40_775), 0).is_some());
        assert_eq!(ancestor_problem(facts(Kind::Directory, 0, 0o40_755), 0), None);
        assert_eq!(ancestor_problem(facts(Kind::Directory, 0, 0o41_777), 0), None, "sticky");
        assert!(ancestor_problem(facts(Kind::Directory, SERVICE_UID, 0o40_755), 0).is_some());
        assert!(ancestor_problem(facts(Kind::Directory, 0, 0o40_777), 0).is_some());
        assert!(ancestor_problem(facts(Kind::Directory, SERVICE_UID, 0o41_777), 0).is_some());
    }

    /// 非 root 运行只用于不跨账号的隔离环境：安装目录必须归当前账号，程序与祖先可归当前账号或
    /// root，其他账号一律拒绝。
    #[test]
    fn non_root_trusts_itself_and_root() {
        let euid = 1000;
        assert_eq!(program_problem(facts(Kind::File, euid, 0o100_755), euid), None);
        assert_eq!(program_problem(facts(Kind::File, 0, 0o100_755), euid), None);
        assert!(program_problem(facts(Kind::File, SERVICE_UID, 0o100_755), euid).is_some());
        assert_eq!(directory_problem(facts(Kind::Directory, euid, 0o40_755), euid), None);
        assert!(directory_problem(facts(Kind::Directory, 0, 0o40_755), euid).is_some());
        assert_eq!(ancestor_problem(facts(Kind::Directory, euid, 0o40_700), euid), None);
    }

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("auction-trust-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).expect("建立临时目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("设置权限");
            Self(root)
        }

        fn install(&self) -> PathBuf {
            let install = self.0.join("bin");
            fs::create_dir(&install).expect("建立安装目录");
            fs::set_permissions(&install, fs::Permissions::from_mode(0o755)).expect("设置权限");
            for name in REQUIRED_BINARIES {
                let path = install.join(name);
                fs::write(&path, b"program").expect("写入程序");
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("设置权限");
            }
            install
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 真实目录：安全布局通过；可写祖先、可写安装目录、可写程序、符号链接程序与符号链接安装
    /// 目录都在停机前拒绝，拒绝前不修改任何文件。
    #[test]
    fn install_dir_gate_reads_real_directories() {
        let root = TempRoot::new();
        let install = root.install();
        require_install_dir(&install).expect("安全布局通过");
        PinnedInstallDir::open(&install).expect("固定安装目录");

        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o775)).expect("放宽祖先");
        let text = format!("{:#}", require_install_dir(&install).expect_err("可写祖先拒绝"));
        assert!(text.contains("祖先") && text.contains("重建安装目录信任"), "{text}");
        assert!(PinnedInstallDir::open(&install).is_err(), "安装时同样核对祖先");
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o755)).expect("恢复祖先");

        fs::set_permissions(&install, fs::Permissions::from_mode(0o775)).expect("放宽目录");
        assert!(require_install_dir(&install).is_err(), "group 可写安装目录拒绝");
        fs::set_permissions(&install, fs::Permissions::from_mode(0o755)).expect("恢复目录");

        let worker = install.join("auction-worker");
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o775)).expect("放宽程序");
        let text = format!("{:#}", require_install_dir(&install).expect_err("可写程序拒绝"));
        assert!(text.contains("auction-worker") && text.contains("不会自动修补"), "{text}");
        assert_eq!(
            fs::metadata(&worker).expect("程序元数据").permissions().mode() & 0o777,
            0o775,
            "门禁不修补权限"
        );
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o755)).expect("恢复程序");

        fs::remove_file(&worker).expect("移除程序");
        std::os::unix::fs::symlink(install.join("auction-cli"), &worker).expect("符号链接程序");
        assert!(require_install_dir(&install).is_err(), "符号链接程序拒绝");

        let link = root.0.join("link");
        std::os::unix::fs::symlink(&install, &link).expect("符号链接安装目录");
        assert!(require_install_dir(&link).is_err(), "符号链接安装目录拒绝");
        assert!(PinnedInstallDir::open(&link).is_err());
        assert!(require_install_dir(&root.0.join("bin/../bin")).is_err(), "拒绝 ..");
    }
}
