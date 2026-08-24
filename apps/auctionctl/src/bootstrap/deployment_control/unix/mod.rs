//! Unix/systemd 部署控制器的模块装配。
//!
//! 生命周期、状态模型和安全校验分别由子模块负责；本文件只保留父部署控制器使用的
//! 稳定包装，不改变 systemd 启停顺序、readiness 或失败回滚契约。

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
