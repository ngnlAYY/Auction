//! Unix/systemd 部署控制器的模块装配；启停顺序、readiness 与失败回滚契约由子模块保持。

use std::{path::PathBuf, time::Duration};

use super::ControllerOptions;

#[path = "controller.rs"]
mod controller;
#[path = "lifecycle.rs"]
mod lifecycle;
#[path = "model.rs"]
mod model;
#[path = "validation.rs"]
mod validation;

/// 独立数据库维护持有的部署锁，生命周期覆盖 migration 或 bootstrap。
pub(in crate::bootstrap) struct MigrationGuard {
    _lock: model::DeploymentLock,
}

pub(super) fn acquire_migration_guard(
    options: ControllerOptions,
) -> anyhow::Result<MigrationGuard> {
    let controller = model::Controller::new(options)?;
    let lock = controller.acquire_lock()?;
    let snapshots = controller.snapshots()?;
    let running_roles = super::systemd::START_ORDER
        .into_iter()
        .filter(|role| {
            snapshots.get(role).is_none_or(|snapshot| !snapshot.safely_stopped_for_migration())
        })
        .map(super::systemd::ManagedRole::name)
        .collect::<Vec<_>>();
    if !running_roles.is_empty() {
        anyhow::bail!(
            "独立数据库维护要求先停止全部进程；检测到运行或状态转换中的角色：{}",
            running_roles.join(", ")
        )
    }
    Ok(MigrationGuard { _lock: lock })
}

pub(super) async fn start(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> anyhow::Result<()> {
    lifecycle::start(options, config_dir, startup_timeout).await
}

pub(super) fn stop(options: ControllerOptions) -> anyhow::Result<()> {
    lifecycle::stop(options)
}

pub(super) async fn restart(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> anyhow::Result<()> {
    lifecycle::restart(options, config_dir, startup_timeout).await
}

pub(super) fn status(options: ControllerOptions) -> anyhow::Result<()> {
    lifecycle::status(options)
}
