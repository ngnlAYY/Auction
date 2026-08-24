//! systemd 三进程部署控制器。
//!
//! systemd 继续拥有单个进程的 PID、信号、停止超时和 journald 日志；这里恢复旧版控制器的
//! 跨角色契约：严格预检、Worker → Admin → Public 启动顺序、应用 readiness、失败回滚、
//! Public → Admin → Worker 停止顺序、互斥控制以及不包含敏感配置的启动回执。

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
