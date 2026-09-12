//! Unix/systemd 部署控制器的跨角色启动、停止、重启和状态编排。

use std::{path::PathBuf, time::Duration};

use super::super::ControllerOptions;
use super::super::receipt::receipt_match;
use super::super::systemd::{START_ORDER, single_line};
use super::model::{Controller, StartFailure};
use crate::bootstrap::migrate_config;

pub(super) async fn start(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> anyhow::Result<()> {
    let result = start_inner(options, config_dir, startup_timeout).await;
    finish_start_result(result)
}

async fn start_inner(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> Result<(), StartFailure> {
    let controller =
        Controller::new(options).map_err(|error| StartFailure::new("start-prepare", error))?;
    let _lock =
        controller.acquire_lock().map_err(|error| StartFailure::new("start-lock", error))?;
    let prepared = controller
        .prepare(&config_dir)
        .map_err(|error| StartFailure::new("start-prepare", error))?;
    let snapshots =
        controller.snapshots().map_err(|error| StartFailure::new("start-state", error))?;
    let running = snapshots.values().filter(|snapshot| snapshot.running()).count();
    if running == START_ORDER.len() {
        let records = controller
            .observe_ready_roles(&prepared, startup_timeout)
            .await
            .map_err(|error| StartFailure::new("start-state", error))?;
        controller
            .publish_receipt("adopted", records)
            .map_err(|error| StartFailure::new("startup-receipt", error))?;
        println!("deployment already running; readiness and startup receipt refreshed");
        return Ok(());
    }
    if snapshots.values().any(|snapshot| !snapshot.stopped()) {
        return Err(StartFailure::new(
            "start-state",
            anyhow::anyhow!(
                "检测到部分 unit 正在运行或转换状态；请先执行 `auctionctl deployment stop`"
            ),
        ));
    }
    controller
        .ensure_addresses_available(&prepared)
        .map_err(|error| StartFailure::new("address-check", error))?;
    migrate_config(&prepared.migration_config)
        .await
        .map_err(|error| StartFailure::new("migration", error))?;
    controller.launch(&prepared, startup_timeout).await
}

pub(super) fn stop(options: ControllerOptions) -> anyhow::Result<()> {
    let controller = Controller::new(options)?;
    let _lock = controller.acquire_lock()?;
    controller.stop_all()?;
    controller.remove_receipt()?;
    println!("deployment stopped");
    Ok(())
}

pub(super) async fn restart(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> anyhow::Result<()> {
    let result = restart_inner(options, config_dir, startup_timeout).await;
    finish_start_result(result)
}

async fn restart_inner(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> Result<(), StartFailure> {
    let controller =
        Controller::new(options).map_err(|error| StartFailure::new("start-prepare", error))?;
    let _lock =
        controller.acquire_lock().map_err(|error| StartFailure::new("start-lock", error))?;
    // 与旧版一致，先证明新配置和 unit 契约可用，再停止当前服务。
    let prepared = controller
        .prepare(&config_dir)
        .map_err(|error| StartFailure::new("start-prepare", error))?;
    controller.stop_all().map_err(|error| StartFailure::new("restart-stop", error))?;
    controller.remove_receipt().map_err(|error| StartFailure::new("restart-stop", error))?;
    controller
        .ensure_addresses_available(&prepared)
        .map_err(|error| StartFailure::new("address-check", error))?;
    migrate_config(&prepared.migration_config)
        .await
        .map_err(|error| StartFailure::new("migration", error))?;
    controller.launch(&prepared, startup_timeout).await
}

pub(super) fn status(options: ControllerOptions) -> anyhow::Result<()> {
    let controller = Controller::new(options)?;
    let receipt = controller.read_receipt()?;
    let mut healthy = true;
    for role in START_ORDER {
        match controller.snapshot(role) {
            Ok(snapshot) => {
                let receipt_state = receipt
                    .as_ref()
                    .map(|receipt| receipt_match(receipt, role, controller.unit(role), &snapshot))
                    .unwrap_or("missing");
                println!(
                    "role={} unit={} load_state={} active_state={} sub_state={} main_pid={} receipt={}",
                    role.name(),
                    controller.unit(role),
                    snapshot.load_state,
                    snapshot.active_state,
                    snapshot.sub_state,
                    snapshot.main_pid,
                    receipt_state,
                );
                if !snapshot.running() || receipt_state == "stale" {
                    healthy = false;
                }
            }
            Err(error) => {
                healthy = false;
                println!(
                    "role={} unit={} load_state=unknown active_state=unknown error={}",
                    role.name(),
                    controller.unit(role),
                    single_line(&error.to_string(), 512),
                );
            }
        }
    }
    if healthy { Ok(()) } else { anyhow::bail!("deployment status is not fully healthy") }
}

fn finish_start_result(result: Result<(), StartFailure>) -> anyhow::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(failure) => {
            eprintln!("[auction] start-failure-v1={}", failure.code);
            Err(failure.error)
        }
    }
}
