//! 写数据库的维护命令的停机门禁：合并部署以 owner.lock 证明 supervisor 已退出，
//! 分体部署以 systemd 状态证明三角色均已停止，并与 service 命令共用部署锁。

use std::fs::File;

use anyhow::Context;

use super::systemd::{ManagedRole, Systemctl};
use crate::cli::MaintenanceArgs;
use crate::deployment::ConfigDir;
use crate::support::files;

pub(super) const DEPLOYMENT_LOCK: &str = "deployment-control.lock";
pub(super) const DEPLOYMENT_BUSY: &str = "另一个 auction-cli 部署操作正在进行";

/// 持有期间阻止 supervisor 或另一个部署操作启动；Drop 时释放全部锁。
pub(crate) struct MaintenanceGuard {
    _locks: Vec<File>,
    pub(crate) mode: &'static str,
}

/// 合并部署的 supervisor 运行目录锁；存在即表示该配置目录由 auction-server 管理。
pub(super) fn managed_lock(config_dir: &ConfigDir) -> anyhow::Result<Option<File>> {
    let runtime = auction_config::managed_runtime_directory(config_dir.path())
        .map_err(|_| anyhow::anyhow!("无法推导合并部署运行目录"))?;
    files::lock_existing(
        &runtime.join("owner.lock"),
        "auction-server 仍在运行；请先停止合并部署服务并等待全部子进程退出",
    )
}

pub(crate) async fn acquire(
    config_dir: &ConfigDir,
    args: &MaintenanceArgs,
) -> anyhow::Result<MaintenanceGuard> {
    if let Some(lock) = managed_lock(config_dir)? {
        crate::app::progress("检测到合并部署 owner.lock，已取得独占锁");
        return Ok(MaintenanceGuard { _locks: vec![lock], mode: "managed" });
    }
    if args.no_systemd {
        crate::app::progress("已声明 --no-systemd：跳过 unit 停止检查，请自行确保没有角色在运行");
        return Ok(MaintenanceGuard { _locks: Vec::new(), mode: "unmanaged" });
    }
    let systemctl = Systemctl::new(&args.units)?;
    let locks = vec![deployment_lock(&args.units)?];
    let mut running = Vec::new();
    for role in ManagedRole::START_ORDER {
        let state = systemctl.show(role).await?;
        if !state.stopped() {
            running.push(format!("{}={}/{}", role.name(), state.active_state, state.sub_state));
        }
    }
    if !running.is_empty() {
        anyhow::bail!(
            "维护操作要求三角色均已停止；运行或转换中的角色：{}（先执行 auction-cli service stop）",
            running.join(", ")
        );
    }
    Ok(MaintenanceGuard { _locks: locks, mode: "systemd" })
}

/// 分体部署启停共用的部署锁；状态目录不存在时以 0700 创建。
pub(crate) fn deployment_lock(args: &crate::cli::UnitArgs) -> anyhow::Result<File> {
    files::ensure_private_dir(&args.state_dir).context("准备部署状态目录失败")?;
    files::lock_file(&args.state_dir.join(DEPLOYMENT_LOCK), DEPLOYMENT_BUSY)
}
