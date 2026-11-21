//! 安装目录的信任边界与以描述符固定的安装操作。
//!
//! root 与 postgres 都会执行安装目录中的 auction-cli（部署编排、`archive_command`），因此 root
//! 运行时安装目录、它的每一级祖先与其中的程序都必须归 root 所有、不是符号链接且禁止 group/other
//! 写入，新程序一律 root:root 0755。非 root 运行只用于不跨账号的隔离环境：可信属主为当前账号，
//! 祖先与既有程序也可以归 root。门禁只读、不修补：不安全的既有安装不会被自动 chown，须由维护者
//! 用已验证的 Release 重建信任。
//!
//! 三角色 unit 以各自的运行账号执行程序：安装目录与每一级祖先必须能被它们穿越，各角色程序必须
//! 能被对应账号执行。root 0700 的目录、ACL 布局与 noexec 挂载都在停机前拒绝，不自动 chmod。
//!
//! 执行本命令的账号（部署执行账号）同样要执行安装目录中的旧 auction-cli：deploy 停机后用它校验
//! 停机备份，停机后失败的恢复指引也用它执行 `service start`。按当前有效 UID 与固定 mode 位做
//! 保守判定；ACL 只在已知本地文件系统上可证明命名空间不适用时忽略，未知或网络文件系统失败关闭。
//!
//! 安装、换回与复核都相对同一个以 `O_DIRECTORY | O_NOFOLLOW` 打开的目录描述符完成：临时文件以
//! `O_EXCL | O_NOFOLLOW` 新建，`renameat` 原子替换，替换后重新打开目标复核属主、权限与摘要。

use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};

use anyhow::Context;
use uuid::Uuid;

use super::super::artifact::REQUIRED_BINARIES;
use crate::support::digest::sha256_reader;
use crate::support::files::{self, Facts, Kind, ancestor_problem};

/// 安装目录中程序的唯一权限。
pub(crate) const PROGRAM_MODE: u32 = 0o755;
/// 门禁拒绝时的处置：不自动修补，由维护者从已验证的 Release 重建信任。
const REBUILD_TRUST: &str = "不会自动修补属主或权限：请按 ops/deploy/RECOVERY.md 的“重建安装目录信任”\
                             用已验证的 Release 重装为 root:root 0755 后重试";
/// 只缺执行位时的处置：属主与写权限已核对可信、内容没有被他人改写的途径，补上执行位即可。
const STANDARD_ACCESS: &str = "不会自动修改权限：属主与写权限已核对可信，按 ops/README.md 的“目录与\
                               账号”把安装目录、各级父目录与程序设为标准的 0755 后重试";

/// 三角色 unit 的运行账号与它执行的程序；`uid` 为 None 表示 `DynamicUser=yes`（UID 由 systemd
/// 动态分配，不可能是这些目录与程序的属主）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunAccount {
    pub(crate) role: &'static str,
    pub(crate) program: &'static str,
    pub(crate) uid: Option<u32>,
}

/// owner UID 以属主执行位判定；其他 UID 要求 group 与 other 同时有执行位，保守覆盖未知组成员关系。
/// 安装检查还会拒绝 POSIX ACL 并检查程序挂载 noexec 标志，因此 mode 位是已界定布局下的充分证明。
fn executable_by(facts: Facts, uid: Option<u32>) -> bool {
    let required = if uid == Some(facts.uid) { 0o100 } else { 0o011 };
    facts.mode & required == required
}

fn account_text(account: &RunAccount) -> String {
    match account.uid {
        Some(uid) => format!("{} 的运行账号（UID {uid}）", account.role),
        None => format!("{} 的运行账号（DynamicUser）", account.role),
    }
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
/// 四个程序的信任、三角色运行账号与当前部署账号的执行权限；ACL/`noexec` 布局失败关闭。
pub(crate) fn require_install_dir(path: &Path, accounts: &[RunAccount]) -> anyhow::Result<()> {
    let directory = PinnedInstallDir::open(path)?;
    for (prefix, facts) in &directory.levels {
        if let Some(account) = accounts.iter().find(|account| !executable_by(*facts, account.uid)) {
            anyhow::bail!(
                "{}无法进入 {}（缺少执行位），停机替换后角色将无法启动；{STANDARD_ACCESS}",
                account_text(account),
                prefix.display()
            );
        }
    }
    for name in REQUIRED_BINARIES {
        let program = directory.open_program(name)?;
        let metadata = program.metadata().context("读取当前安装程序元数据失败")?;
        let facts = Facts::of(&metadata);
        if let Some(problem) = program_problem(facts, directory.euid) {
            anyhow::bail!("安装目录中的 {name} 不可信（{problem}）；{REBUILD_TRUST}");
        }
        if let Some(account) = accounts.iter().find(|account| account.program == name)
            && !executable_by(facts, account.uid)
        {
            anyhow::bail!(
                "安装目录中的 {name} 不能由 {} 执行（缺少执行位）；{STANDARD_ACCESS}",
                account_text(account)
            );
        }
        // 三角色停止之后才会启动旧 CLI（deploy 的停机备份校验、失败后的 service start）：
        // 部署执行账号执行不了它时只能在这里拒绝，否则服务已停才失败。
        if name == "auction-cli" && !executable_by(facts, Some(directory.euid)) {
            anyhow::bail!(
                "安装目录中的 auction-cli 不能由部署执行账号（UID {}）执行（缺少可信执行位）；{STANDARD_ACCESS}",
                directory.euid
            );
        }
    }
    Ok(())
}

/// 安装目录与每一级祖先；能替换其中任一目录项的账号就能替换 root 执行的程序。返回从根到安装
/// 目录逐级的路径与元数据事实，供运行账号的穿越核对使用。
fn require_trusted_directory(path: &Path) -> anyhow::Result<(Vec<(PathBuf, Facts)>, File)> {
    if !path.is_absolute()
        || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        anyhow::bail!("安装目录必须是不含 . 或 .. 的绝对路径：{}", path.display());
    }
    let euid = files::effective_uid();
    let components = path
        .components()
        .filter_map(|part| match part {
            Component::RootDir => None,
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")
        .context("打开根目录以固定安装目录祖先失败")?;
    let mut prefix = PathBuf::from("/");
    let mut levels = Vec::with_capacity(components.len() + 1);
    let root_facts = Facts::of(&directory.metadata().context("读取根目录元数据失败")?);
    let root_problem = if components.is_empty() {
        directory_problem(root_facts, euid)
    } else {
        ancestor_problem(root_facts, euid)
    };
    if let Some(problem) = root_problem {
        anyhow::bail!("安装目录祖先 / 不可信（{problem}）；{REBUILD_TRUST}");
    }
    require_no_acl(&directory, &prefix, true)?;
    levels.push((prefix.clone(), root_facts));

    for (index, component) in components.iter().enumerate() {
        prefix.push(component);
        let name = CString::new(component.as_bytes()).context("安装目录路径项包含 NUL")?;
        // SAFETY：父目录描述符固定，name 是单个普通路径分量，成功返回的描述符由当前调用独占。
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("打开安装目录祖先失败：{}", prefix.display()));
        }
        // SAFETY：openat 成功，descriptor 由当前调用独占拥有。
        let next = unsafe { File::from_raw_fd(descriptor) };
        let metadata = next
            .metadata()
            .with_context(|| format!("读取安装目录元数据失败：{}", prefix.display()))?;
        let mut entry = std::mem::MaybeUninit::<libc::stat>::zeroed();
        // SAFETY: directory is pinned, name is one NUL-free component, and entry is writable stat storage.
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                entry.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("复核安装目录项失败：{}", prefix.display()));
        }
        // SAFETY: fstatat initialized entry after returning success.
        let entry = unsafe { entry.assume_init() };
        if entry.st_dev != metadata.dev()
            || entry.st_ino != metadata.ino()
            || entry.st_mode & libc::S_IFMT != libc::S_IFDIR
        {
            anyhow::bail!("安装目录项在固定时被替换：{}", prefix.display());
        }
        let facts = Facts::of(&metadata);
        let last = index + 1 == components.len();
        let problem =
            if last { directory_problem(facts, euid) } else { ancestor_problem(facts, euid) };
        if let Some(problem) = problem {
            anyhow::bail!("安装目录 {} 不可信（{problem}）；{REBUILD_TRUST}", prefix.display());
        }
        require_no_acl(&next, &prefix, true)?;
        levels.push((prefix.clone(), facts));
        directory = next;
    }
    Ok((levels, directory))
}

/// ACL 会覆盖目标 UID 的 mode 位；有 ACL 的布局失败关闭，目录默认 ACL 同样拒绝。
/// `EOPNOTSUPP` 仅在明确支持的本地文件系统上表示该 ACL 命名空间不适用；其他文件系统保持失败关闭。
fn require_no_acl(file: &File, path: &Path, directory: bool) -> anyhow::Result<()> {
    const ACCESS_ACL: &[u8] = b"system.posix_acl_access\0";
    const DEFAULT_ACL: &[u8] = b"system.posix_acl_default\0";
    const NFS4_ACL: &[u8] = b"system.nfs4_acl\0";
    const RICH_ACL: &[u8] = b"system.richacl\0";
    let attributes = if directory {
        &[ACCESS_ACL, DEFAULT_ACL, NFS4_ACL, RICH_ACL][..]
    } else {
        &[ACCESS_ACL, NFS4_ACL, RICH_ACL][..]
    };
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY：stat 指向可写的 libc::statfs 存储，file 是打开的有效描述符。
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("读取安装路径文件系统类型失败：{}", path.display()));
    }
    // SAFETY：fstatfs 成功时已初始化 stat。
    let filesystem_type = unsafe { stat.assume_init() }.f_type;
    for attribute in attributes {
        // SAFETY：描述符有效，属性名是静态 NUL 结尾字符串；size=0 只查询是否存在。
        let size = unsafe {
            libc::fgetxattr(file.as_raw_fd(), attribute.as_ptr().cast(), std::ptr::null_mut(), 0)
        };
        if size >= 0 {
            anyhow::bail!("安装目录或程序含访问 ACL，无法证明运行账号访问权限：{}", path.display());
        }
        let error = std::io::Error::last_os_error();
        let errno = error.raw_os_error().unwrap_or(libc::EIO);
        match classify_acl_probe(Err(errno), filesystem_type) {
            AclProbeResult::Absent => continue,
            AclProbeResult::Opaque => anyhow::bail!(
                "安装路径所在文件系统无法证明 ACL 命名空间不存在（类型 {filesystem_type:#x}）：{}",
                path.display()
            ),
            AclProbeResult::Failed(_) => {
                return Err(error)
                    .with_context(|| format!("读取安装路径 ACL 失败：{}", path.display()));
            }
            AclProbeResult::Present => anyhow::bail!(
                "安装目录或程序含访问 ACL，无法证明运行账号访问权限：{}",
                path.display()
            ),
        }
    }
    Ok(())
}

/// Linux 文件系统 magic：仅这些本地文件系统可把“不支持该 xattr 命名空间”视为该 ACL 形式不适用。
fn known_local_filesystem(filesystem_type: libc::c_long) -> bool {
    matches!(
        filesystem_type,
        0x0000_ef53 // ext2/3/4
            | 0x5846_5342 // XFS
            | 0x9123_683e // Btrfs
            | 0x0102_1994 // tmpfs
            | 0x794c_7630 // overlayfs
            | 0x8584_58f6 // ramfs
            | 0xf2f5_2010 // F2FS
    )
}

#[derive(Debug, PartialEq, Eq)]
enum AclProbeResult {
    Present,
    Absent,
    Opaque,
    Failed(i32),
}

fn classify_acl_probe(result: Result<isize, i32>, filesystem_type: libc::c_long) -> AclProbeResult {
    match result {
        Ok(_) => AclProbeResult::Present,
        Err(errno) if errno == libc::ENODATA => AclProbeResult::Absent,
        Err(errno) if errno == libc::EOPNOTSUPP && known_local_filesystem(filesystem_type) => {
            AclProbeResult::Absent
        }
        Err(errno) if errno == libc::EOPNOTSUPP => AclProbeResult::Opaque,
        Err(errno) => AclProbeResult::Failed(errno),
    }
}

/// 程序文件所在挂载的 noexec 标记不能从 mode 位获知，须从同一文件描述符读取挂载属性。
fn require_executable_mount(file: &File, path: &Path) -> anyhow::Result<()> {
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY：stat 指向可写的 libc::statvfs 存储。
    if unsafe { libc::fstatvfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("读取安装程序文件系统属性失败：{}", path.display()));
    }
    // SAFETY: fstatvfs initialized stat on success.
    let flags = unsafe { stat.assume_init() }.f_flag;
    if flags & libc::ST_NOEXEC as libc::c_ulong != 0 {
        anyhow::bail!("安装程序所在文件系统使用 noexec：{}", path.display());
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
    levels: Vec<(PathBuf, Facts)>,
    euid: u32,
}

impl PinnedInstallDir {
    /// 安装与换回前重新核对安装目录与祖先，再在打开的描述符上复核目录本身。程序本身的信任在
    /// 停机前由 [`require_install_dir`] 核对；这里不重查，换回时目标可能已被部分替换。
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
        let (levels, directory) = require_trusted_directory(path)?;
        Ok(Self { path: path.to_owned(), directory, levels, euid: files::effective_uid() })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// 新建同目录临时程序（`O_EXCL`、0755）；返回临时名与可写描述符。
    pub(crate) fn create_temporary(&self, name: &str) -> anyhow::Result<(String, File)> {
        let temporary = format!(".{name}.{}.new", Uuid::new_v4());
        let file = self
            .open_at(&temporary, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL, PROGRAM_MODE)
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

    fn open_program(&self, name: &str) -> anyhow::Result<File> {
        let file = self
            .open_at(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)
            .with_context(|| format!("打开安装目录中的程序失败：{name}"))?;
        let metadata = file.metadata().context("读取程序元数据失败")?;
        if !metadata.is_file() {
            anyhow::bail!("安装目录中的 {name} 不是普通文件");
        }
        let path = self.path.join(name);
        require_no_acl(&file, &path, false)?;
        require_executable_mount(&file, &path)?;
        Ok(file)
    }

    /// 重新打开目录中的程序并读取事实；不是普通文件时失败。
    pub(crate) fn inspect(&self, name: &str) -> anyhow::Result<Inspection> {
        let file = self.open_program(name)?;
        let metadata = file.metadata().context("读取程序元数据失败")?;
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::{
        AclProbeResult, Facts, Kind, PinnedInstallDir, RunAccount, ancestor_problem,
        classify_acl_probe, directory_problem, executable_by, program_problem, require_install_dir,
    };
    use crate::deploy::artifact::REQUIRED_BINARIES;
    use crate::deploy::gates::verify_backup_with_cli;
    use crate::support::files::effective_uid;

    const SERVICE_UID: u32 = 990;

    fn facts(kind: Kind, uid: u32, mode: u32) -> Facts {
        Facts { kind, uid, mode }
    }

    /// 三角色都以 `uid` 运行（标准布局里是不拥有安装目录的服务账号）。
    fn accounts(uid: Option<u32>) -> [RunAccount; 3] {
        [
            RunAccount { role: "worker", program: "auction-worker", uid },
            RunAccount { role: "admin", program: "auction-admin-api", uid },
            RunAccount { role: "public", program: "auction-public-api", uid },
        ]
    }

    /// 运行账号的穿越与执行（root 口径的合成元数据）：root:root 0755 的目录与程序可用；root 0700
    /// 的目录、缺执行位的程序对服务账号不可用；group 位缺执行位时即使 other 有也拒绝（账号可能
    /// 正在属组里）；属主账号只看属主位；DynamicUser 只能按非属主判定。
    #[test]
    fn run_account_needs_traverse_and_execute_bits() {
        let service = Some(SERVICE_UID);
        assert!(executable_by(facts(Kind::Directory, 0, 0o40_755), service));
        assert!(executable_by(facts(Kind::Directory, 0, 0o40_711), service));
        assert!(!executable_by(facts(Kind::Directory, 0, 0o40_700), service), "root 0700");
        assert!(!executable_by(facts(Kind::Directory, 0, 0o40_705), service), "group 无执行位");
        assert!(executable_by(facts(Kind::File, 0, 0o100_755), service));
        assert!(!executable_by(facts(Kind::File, 0, 0o100_744), service), "不可执行程序");
        assert!(!executable_by(facts(Kind::File, 0, 0o100_644), service));
        assert!(executable_by(facts(Kind::Directory, 0, 0o40_700), Some(0)), "root 运行的 unit");
        assert!(executable_by(facts(Kind::File, 1000, 0o100_700), Some(1000)), "属主看属主位");
        assert!(executable_by(facts(Kind::File, 0, 0o100_755), None), "DynamicUser");
        assert!(!executable_by(facts(Kind::Directory, 0, 0o40_750), None));
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
        let accounts = accounts(Some(SERVICE_UID));
        require_install_dir(&install, &accounts).expect("安全布局通过");
        PinnedInstallDir::open(&install).expect("固定安装目录");

        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o775)).expect("放宽祖先");
        let text =
            format!("{:#}", require_install_dir(&install, &accounts).expect_err("可写祖先拒绝"));
        assert!(
            text.contains(&root.0.display().to_string()) && text.contains("重建安装目录信任"),
            "{text}"
        );
        assert!(PinnedInstallDir::open(&install).is_err(), "安装时同样核对祖先");
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o755)).expect("恢复祖先");

        fs::set_permissions(&install, fs::Permissions::from_mode(0o775)).expect("放宽目录");
        assert!(require_install_dir(&install, &accounts).is_err(), "group 可写安装目录拒绝");
        fs::set_permissions(&install, fs::Permissions::from_mode(0o755)).expect("恢复目录");

        let worker = install.join("auction-worker");
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o775)).expect("放宽程序");
        let text =
            format!("{:#}", require_install_dir(&install, &accounts).expect_err("可写程序拒绝"));
        assert!(text.contains("auction-worker") && text.contains("不会自动修补"), "{text}");
        assert_eq!(
            fs::metadata(&worker).expect("程序元数据").permissions().mode() & 0o777,
            0o775,
            "门禁不修补权限"
        );
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o755)).expect("恢复程序");

        fs::remove_file(&worker).expect("移除程序");
        std::os::unix::fs::symlink(install.join("auction-cli"), &worker).expect("符号链接程序");
        assert!(require_install_dir(&install, &accounts).is_err(), "符号链接程序拒绝");

        let link = root.0.join("link");
        std::os::unix::fs::symlink(&install, &link).expect("符号链接安装目录");
        assert!(require_install_dir(&link, &accounts).is_err(), "符号链接安装目录拒绝");
        assert!(PinnedInstallDir::open(&link).is_err());

        let ancestor_link = root.0.join("ancestor-link");
        std::os::unix::fs::symlink(&root.0, &ancestor_link).expect("符号链接祖先");
        let redirected = ancestor_link.join("bin");
        assert!(require_install_dir(&redirected, &accounts).is_err(), "符号链接祖先拒绝");
        assert!(PinnedInstallDir::open(&redirected).is_err(), "安装固定路径拒绝符号链接祖先");
        assert!(require_install_dir(&root.0.join("bin/../bin"), &accounts).is_err(), "拒绝 ..");
    }

    /// 属主与写权限都可信、只是服务账号无法穿越或执行：0700 的安装目录或祖先、缺执行位的角色
    /// 程序都在停机前拒绝并点名角色与路径，不修改权限（旧门禁只看属主与写权限，会放行它们，
    /// 停机替换后角色才启动失败）。安装目录归运行账号自己时只看属主位。
    #[test]
    fn unreachable_directories_and_unexecutable_programs_are_rejected() {
        let root = TempRoot::new();
        let install = root.install();
        let service = accounts(Some(SERVICE_UID));
        let mode =
            |path: &PathBuf| fs::metadata(path).expect("元数据").permissions().mode() & 0o777;

        fs::set_permissions(&install, fs::Permissions::from_mode(0o700)).expect("收紧安装目录");
        let text = format!("{:#}", require_install_dir(&install, &service).expect_err("0700 目录"));
        assert!(text.contains("worker 的运行账号（UID 990）无法进入"), "{text}");
        assert!(text.contains(&install.display().to_string()), "{text}");
        assert!(text.contains("不会自动修改权限"), "{text}");
        assert_eq!(mode(&install), 0o700, "门禁不修补权限");
        let owner = accounts(Some(effective_uid()));
        require_install_dir(&install, &owner).expect("运行账号是属主时只看属主位");
        fs::set_permissions(&install, fs::Permissions::from_mode(0o755)).expect("恢复安装目录");

        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).expect("收紧祖先");
        let text = format!("{:#}", require_install_dir(&install, &service).expect_err("0700 祖先"));
        assert!(
            text.contains(&root.0.display().to_string()) && text.contains("无法进入"),
            "{text}"
        );
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o755)).expect("恢复祖先");

        let admin = install.join("auction-admin-api");
        fs::set_permissions(&admin, fs::Permissions::from_mode(0o744)).expect("去掉执行位");
        let text = format!("{:#}", require_install_dir(&install, &service).expect_err("不可执行"));
        assert!(text.contains("auction-admin-api 不能由 admin 的运行账号"), "{text}");
        assert_eq!(mode(&admin), 0o744, "门禁不修补权限");
        let dynamic = accounts(None);
        assert!(require_install_dir(&install, &dynamic).is_err(), "DynamicUser 同样拒绝");
        fs::set_permissions(&admin, fs::Permissions::from_mode(0o755)).expect("恢复程序");
        require_install_dir(&install, &service).expect("恢复标准权限后通过");
    }

    /// 只有旧 auction-cli 缺执行位（0644）：目录、三个角色程序与各类运行账号都满足，部署执行账号
    /// （当前进程）却执行不了停机后要用来校验停机备份的旧 CLI。门禁拒绝并点名，不修改权限；停机后
    /// 的 `verify_backup_with_cli` 在同一权限下确实启动不了它。补回标准 0755 后门禁放行，同一调用
    /// 也能执行它。旧门禁只核对三个角色程序的执行位，会把 0644 的 CLI 一路放行到停机之后。
    #[tokio::test]
    async fn installed_cli_must_be_executable_by_the_deploying_account() {
        let root = TempRoot::new();
        let install = root.install();
        let cli = install.join("auction-cli");
        // 停机备份校验以固定参数启动旧 CLI；用忽略参数、直接成功退出的脚本代替真实程序。
        fs::write(&cli, "#!/bin/sh\nexit 0\n").expect("写入 CLI");
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o644)).expect("去掉执行位");
        let manifest = root.0.join("auction-deploy.dump.manifest");

        for run in [accounts(Some(SERVICE_UID)), accounts(Some(effective_uid())), accounts(None)] {
            let text = format!(
                "{:#}",
                require_install_dir(&install, &run).expect_err("只有 CLI 缺执行位")
            );
            assert!(text.contains("auction-cli 不能由部署执行账号"), "{text}");
            assert!(text.contains(&format!("UID {}", effective_uid())), "{text}");
            assert!(text.contains("不会自动修改权限"), "{text}");
        }
        let mode = fs::metadata(&cli).expect("元数据").permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "门禁不修补权限");
        let spawn = verify_backup_with_cli(&cli, &root.0, &manifest, "auction")
            .await
            .expect_err("0644 的旧 CLI 无法启动");
        let denied = spawn
            .chain()
            .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
            .any(|cause| cause.kind() == std::io::ErrorKind::PermissionDenied);
        assert!(denied, "停机后启动旧 CLI 因权限失败：{spawn:#}");

        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).expect("恢复标准权限");
        require_install_dir(&install, &accounts(Some(SERVICE_UID))).expect("补回 0755 后放行");
        verify_backup_with_cli(&cli, &root.0, &manifest, "auction")
            .await
            .expect("门禁放行时停机后能执行旧 CLI");
    }
    #[test]
    fn acl_probe_preserves_detected_acls_and_absent_attributes() {
        let ext_filesystem = 0x0000_ef53;
        assert_eq!(classify_acl_probe(Ok(0), ext_filesystem), AclProbeResult::Present);
        assert_eq!(
            classify_acl_probe(Err(libc::ENODATA), 0x6969),
            AclProbeResult::Absent,
            "明确不存在的属性在任何文件系统上都是 absent"
        );
    }

    #[test]
    fn unsupported_acl_namespaces_are_ignored_only_on_known_local_filesystems() {
        for filesystem_type in [
            0x0000_ef53, // ext2/3/4
            0x5846_5342, // XFS
            0x9123_683e, // Btrfs
            0x0102_1994, // tmpfs
            0x794c_7630, // overlayfs
            0x8584_58f6, // ramfs
            0xf2f5_2010, // F2FS
        ] {
            assert_eq!(
                classify_acl_probe(Err(libc::EOPNOTSUPP), filesystem_type),
                AclProbeResult::Absent,
                "known local filesystem {filesystem_type:#x}"
            );
        }
    }

    #[test]
    fn unsupported_acl_namespaces_on_opaque_filesystems_fail_closed() {
        for filesystem_type in [0x6969, 0x6573_554e, 0x1234_5678] {
            assert_eq!(
                classify_acl_probe(Err(libc::EOPNOTSUPP), filesystem_type),
                AclProbeResult::Opaque,
                "unknown or network filesystem {filesystem_type:#x}"
            );
        }
        assert_eq!(
            classify_acl_probe(Err(libc::EACCES), 0x0000_ef53),
            AclProbeResult::Failed(libc::EACCES)
        );
    }
}
