//! 分体部署的控制目录（`--state-dir`，标准布局 `/var/lib/auction/deployment`）：部署锁、回执
//! （`receipts/`）与回滚记录（`rollback/`）都经同一个以描述符固定的目录访问。
//!
//! 不变量：目录本身归当前账号（root 运行即 root）且禁止 group/other 访问；路径上的每一级祖先归
//! root 或当前账号且禁止 group/other 写入（root 的 sticky 目录除外）。能改写某一级祖先的账号就能
//! 把整个控制目录改名并换上另一棵树：已持有的锁仍在旧 inode 上，第二个部署进程会在新目录取得
//! 另一把锁，按路径读取的回执、回滚记录与保留程序也会落到另一棵树。因此取锁前核对祖先，此后锁、
//! 回执、回滚记录与保留程序都相对固定的描述符访问并核对属主与权限；停机前再核对路径仍指向同一
//! 目录，被改名或替换时失败关闭。门禁只读，不自动 chown/chmod。

use std::{fs::File, path::Path};

use anyhow::Context;

use crate::support::files::PrivateDirectory;

/// 部署控制锁的文件名。
pub(super) const DEPLOYMENT_LOCK: &str = "deployment-control.lock";
/// 另一个部署操作持锁时的错误。
pub(super) const DEPLOYMENT_BUSY: &str = "另一个 auction-cli 部署操作正在进行";
/// 状态目录或祖先不可信时的处置：不自动修补。
const STANDARD_LAYOUT: &str = "不会自动修改属主或权限：按 ops/README.md 的“目录与账号”，部署状态\
                               目录归 root 且 0700，各级父目录（如 /var/lib/auction）为 root:root \
                               0755，运行账号的可写数据放在其下各自的子目录";

/// 已取得部署锁的控制目录；锁随本值释放。
pub(crate) struct DeploymentState {
    directory: PrivateDirectory,
    _lock: File,
}

impl DeploymentState {
    /// 核对祖先后打开状态目录（不存在时以 0700 只创建最后一级，父目录须已存在），以描述符固定，
    /// 并在其中以非阻塞方式取得部署锁。
    pub(crate) fn lock(path: &Path) -> anyhow::Result<Self> {
        let directory = PrivateDirectory::open_trusted(path, true)
            .with_context(|| format!("部署状态目录不可信；{STANDARD_LAYOUT}"))?
            .with_context(|| format!("部署状态目录不存在：{}", path.display()))?;
        let lock = directory
            .try_lock(DEPLOYMENT_LOCK)
            .with_context(|| format!("部署锁文件不可信；{STANDARD_LAYOUT}"))?
            .context(DEPLOYMENT_BUSY)?;
        Ok(Self { directory, _lock: lock })
    }

    /// 以描述符固定的状态目录；回执与回滚记录都从这里派生。
    pub(crate) fn directory(&self) -> &PrivateDirectory {
        &self.directory
    }

    /// 停机前的复核：路径上的祖先仍然可信，且路径仍指向持锁时固定的目录。被改名或替换说明另一个
    /// 账号或进程能在持锁期间改动控制目录，继续停机只会让锁、回执与回滚记录分属两棵树。
    pub(crate) fn verify_unchanged(&self) -> anyhow::Result<()> {
        self.directory
            .verify_path()
            .with_context(|| format!("部署状态目录在持锁期间被改名或替换；{STANDARD_LAYOUT}"))
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use uuid::Uuid;

    use super::{DEPLOYMENT_BUSY, DeploymentState};

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("auction-deploy-state-{}", Uuid::new_v4()));
            make_directory(&root, 0o755);
            Self(root)
        }
    }

    /// 显式设定权限，不受测试进程 umask 影响。
    fn make_directory(path: &std::path::Path, mode: u32) {
        fs::create_dir(path).expect("建立目录");
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("设置权限");
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 同一控制目录只能有一个持锁者；状态目录以 0700 创建，父目录必须已存在。
    #[test]
    fn lock_is_exclusive_and_creates_only_the_last_level() {
        let root = TempRoot::new();
        let path = root.0.join("deployment");
        let held = DeploymentState::lock(&path).expect("取得部署锁");
        let mode = fs::metadata(&path).expect("状态目录").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let busy = DeploymentState::lock(&path).err().expect("第二个持锁者必须拒绝");
        assert!(format!("{busy:#}").contains(DEPLOYMENT_BUSY), "{busy:#}");
        drop(held);
        DeploymentState::lock(&path).expect("释放后可再次取得");

        let nested = root.0.join("missing").join("deployment");
        assert!(DeploymentState::lock(&nested).is_err(), "不替调用方建立祖先");
        assert!(!root.0.join("missing").exists());
    }

    /// 祖先允许 group 写入（能改名其中的状态目录）时，在创建或打开任何东西之前拒绝。
    #[test]
    fn writable_ancestor_is_rejected_before_any_lock() {
        let root = TempRoot::new();
        let shared = root.0.join("shared");
        make_directory(&shared, 0o775);
        let path = shared.join("deployment");
        let error = DeploymentState::lock(&path).err().expect("可写祖先必须拒绝");
        let text = format!("{error:#}");
        assert!(text.contains("祖先") && text.contains("目录与账号"), "{text}");
        assert!(!path.exists(), "拒绝前不创建状态目录或锁");
    }

    /// 持锁期间祖先被改名、原路径换上另一棵树（锁分离）：第二个进程在新目录取得了另一把锁，
    /// 但先持锁的一方在停机前的复核中失败关闭，不会继续停机与写入。
    #[test]
    fn renamed_ancestor_is_detected_before_shutdown() {
        let root = TempRoot::new();
        let parent = root.0.join("lib");
        make_directory(&parent, 0o755);
        let path = parent.join("deployment");
        let first = DeploymentState::lock(&path).expect("第一个持锁者");
        first.verify_unchanged().expect("未改动时复核通过");

        fs::rename(&parent, root.0.join("lib.moved")).expect("改名祖先");
        make_directory(&parent, 0o755);
        let second = DeploymentState::lock(&path).expect("新目录里的另一把锁");
        let error = first.verify_unchanged().expect_err("锁分离必须被发现");
        assert!(format!("{error:#}").contains("被改名或替换"), "{error:#}");
        second.verify_unchanged().expect("新持锁者固定的就是当前路径");
    }
}
