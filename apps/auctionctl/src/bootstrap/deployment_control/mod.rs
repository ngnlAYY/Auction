//! systemd 三进程部署控制器；保持严格预检、Worker → Admin → Public 启动、应用
//! readiness 与失败回滚，以及 Public → Admin → Worker 停止顺序。

use std::{path::PathBuf, time::Duration};

#[cfg(unix)]
mod probe;
#[cfg(unix)]
mod receipt;
#[cfg(unix)]
mod systemd;

/// systemd 控制器的宿主机边界；unit 名称作为参数传入，不在业务配置中传播。
#[derive(Debug)]
pub(super) struct ControllerOptions {
    pub systemctl: PathBuf,
    pub state_dir: PathBuf,
    pub worker_unit: String,
    pub admin_unit: String,
    pub public_unit: String,
}

/// 获取独立数据库维护的角色停止证明和部署互斥锁。
#[cfg(unix)]
pub(super) fn acquire_migration_guard(
    options: ControllerOptions,
) -> anyhow::Result<unix::MigrationGuard> {
    unix::acquire_migration_guard(options)
}

#[cfg(unix)]
pub(super) async fn start(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> anyhow::Result<()> {
    unix::start(options, config_dir, startup_timeout).await
}

#[cfg(not(unix))]
pub(super) async fn start(
    _options: ControllerOptions,
    _config_dir: PathBuf,
    _startup_timeout: Duration,
) -> anyhow::Result<()> {
    anyhow::bail!("deployment control 只支持提供 systemd 和 Unix socket 的 Unix 主机")
}

#[cfg(unix)]
pub(super) fn stop(options: ControllerOptions) -> anyhow::Result<()> {
    unix::stop(options)
}

#[cfg(not(unix))]
pub(super) fn stop(_options: ControllerOptions) -> anyhow::Result<()> {
    anyhow::bail!("deployment control 只支持提供 systemd 和 Unix socket 的 Unix 主机")
}

#[cfg(unix)]
pub(super) async fn restart(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> anyhow::Result<()> {
    unix::restart(options, config_dir, startup_timeout).await
}

#[cfg(not(unix))]
pub(super) async fn restart(
    _options: ControllerOptions,
    _config_dir: PathBuf,
    _startup_timeout: Duration,
) -> anyhow::Result<()> {
    anyhow::bail!("deployment control 只支持提供 systemd 和 Unix socket 的 Unix 主机")
}

#[cfg(unix)]
pub(super) fn status(options: ControllerOptions) -> anyhow::Result<()> {
    unix::status(options)
}

#[cfg(not(unix))]
pub(super) fn status(_options: ControllerOptions) -> anyhow::Result<()> {
    anyhow::bail!("deployment control 只支持提供 systemd 和 Unix socket 的 Unix 主机")
}

#[cfg(unix)]
mod unix;
