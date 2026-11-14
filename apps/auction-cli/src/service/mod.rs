//! 分体 systemd 部署的顺序编排：严格预检 → 迁移 → Worker/Admin/Public 逐个启动并等待
//! readiness；任一步失败只回滚本次启动的 unit，不回退已提交的迁移。停止顺序相反。

use std::time::Duration;

use auction_process::probe_ready;
use chrono::Utc;
use serde::Serialize;

use crate::app::{print_json, progress};
use crate::cli::{ServiceAction, ServiceArgs};
use crate::config_dir::ConfigDir;
use crate::receipt::{self, Action, Outcome, ReceiptContext};

mod guard;
mod lifecycle;
mod migration;
mod receipts;
mod shutdown;
mod systemd;
use self::lifecycle::{endpoint, observe};
use self::migration::load_owner;
use self::receipts::{publish_receipt, receipt_summary, record_failure};
pub(crate) use lifecycle::{launch, prepare, stop_all};
pub(crate) use migration::service_uid;

pub(crate) use guard::acquire as acquire_maintenance;
pub(crate) use guard::deployment_lock;
pub(crate) use shutdown::{ShutdownGateReport, check as shutdown_gate};
pub(crate) use systemd::{ManagedRole, Systemctl, UnitState};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Serialize)]
struct RoleStatus {
    role: &'static str,
    unit: String,
    #[serde(flatten)]
    state: Option<UnitState>,
    ready: Option<bool>,
    error: Option<String>,
}

pub(crate) async fn run(config_dir: &ConfigDir, args: ServiceArgs) -> anyhow::Result<()> {
    let ServiceArgs { units, action } = args;
    // 回执查询是纯只读文件操作：不需要 systemctl，也不取部署锁。
    if let ServiceAction::Receipt(receipt) = action {
        return receipt::query(&units.state_dir, receipt);
    }
    let systemctl = Systemctl::new(&units)?;
    match action {
        ServiceAction::Status => status(config_dir, &systemctl).await,
        ServiceAction::Receipt(_) => unreachable!("已在上方处理"),
        ServiceAction::Stop { shutdown } => {
            let _lock = guard::deployment_lock(&units)?;
            let gate = shutdown::check(config_dir, None, "stop", &shutdown).await?;
            stop_all(&systemctl).await?;
            print_json(&serde_json::json!({
                "action": "stop", "stopped": true, "shutdown_gate": gate,
            }))
        }
        ServiceAction::Start { startup_timeout_seconds, owner, receipt } => {
            let context =
                ReceiptContext::resolve(receipt.backup_id.as_deref(), receipt.actor.as_deref())?;
            let owner = load_owner(&owner)?;
            let started_at = Utc::now();
            let _lock = guard::deployment_lock(&units)?;
            let deployment = prepare(config_dir, &systemctl).await?;
            let mut states = Vec::with_capacity(3);
            for role in ManagedRole::START_ORDER {
                states.push(systemctl.show(role).await?);
            }
            let timeout = Duration::from_secs(startup_timeout_seconds);
            if states.iter().all(UnitState::running) {
                progress("三个角色均已运行，只复核 readiness");
                let roles = match observe(&systemctl, &deployment, timeout).await {
                    Ok(roles) => roles,
                    Err(error) => {
                        record_failure(
                            &units,
                            &systemctl,
                            Action::Start,
                            started_at,
                            &context,
                            &error,
                        )
                        .await?;
                        return Err(error);
                    }
                };
                let published = publish_receipt(
                    &units,
                    &systemctl,
                    Action::Start,
                    Outcome::AlreadyRunning,
                    started_at,
                    &context,
                    None,
                    None,
                )
                .await?;
                return print_json(&serde_json::json!({
                    "action": "start", "outcome": "already_running", "roles": roles,
                    "deployment_receipt": receipt_summary(&units, &published),
                }));
            }
            if !states.iter().all(UnitState::stopped) {
                anyhow::bail!("部分角色正在运行或处于转换状态；请先执行 auction-cli service stop");
            }
            match launch(&systemctl, &deployment, Some(&owner), timeout, None).await {
                Ok(report) => {
                    let published = publish_receipt(
                        &units,
                        &systemctl,
                        Action::Start,
                        Outcome::Succeeded,
                        started_at,
                        &context,
                        report.migration.as_ref().map(|migration| migration.record.clone()),
                        None,
                    )
                    .await?;
                    print_json(&report.report_json(&units, &published))
                }
                Err(error) => {
                    record_failure(&units, &systemctl, Action::Start, started_at, &context, &error)
                        .await?;
                    Err(error)
                }
            }
        }
        ServiceAction::Restart { startup_timeout_seconds, owner, shutdown, receipt } => {
            let context =
                ReceiptContext::resolve(receipt.backup_id.as_deref(), receipt.actor.as_deref())?;
            let owner = load_owner(&owner)?;
            let started_at = Utc::now();
            let _lock = guard::deployment_lock(&units)?;
            // 先证明新配置与 unit 绑定可用，再停止当前服务。
            let deployment = prepare(config_dir, &systemctl).await?;
            let gate =
                shutdown::check(config_dir, Some(&deployment.worker), "restart", &shutdown).await?;
            stop_all(&systemctl).await?;
            match launch(
                &systemctl,
                &deployment,
                Some(&owner),
                Duration::from_secs(startup_timeout_seconds),
                Some(serde_json::to_value(&gate)?),
            )
            .await
            {
                Ok(report) => {
                    let published = publish_receipt(
                        &units,
                        &systemctl,
                        Action::Restart,
                        Outcome::Succeeded,
                        started_at,
                        &context,
                        report.migration.as_ref().map(|migration| migration.record.clone()),
                        None,
                    )
                    .await?;
                    print_json(&report.report_json(&units, &published))
                }
                Err(error) => {
                    record_failure(
                        &units,
                        &systemctl,
                        Action::Restart,
                        started_at,
                        &context,
                        &error,
                    )
                    .await?;
                    Err(error)
                }
            }
        }
    }
}

/// 状态查询不取部署锁、不要求严格权限；配置无法加载时仍报告 systemd 状态。
async fn status(config_dir: &ConfigDir, systemctl: &Systemctl) -> anyhow::Result<()> {
    let deployment = crate::config_dir::check(config_dir, false);
    let config_error = deployment.as_ref().err().map(|error| format!("{error:#}"));
    let mut roles = Vec::with_capacity(3);
    let mut healthy = true;
    for role in ManagedRole::START_ORDER {
        let unit = systemctl.unit(role).to_owned();
        match systemctl.show(role).await {
            Ok(state) => {
                let ready = match (&deployment, state.running()) {
                    (Ok(deployment), true) => match endpoint(deployment, role) {
                        Ok((endpoint, path)) => {
                            Some(probe_ready(endpoint.readiness_target(), path).await)
                        }
                        Err(_) => None,
                    },
                    (_, false) => Some(false),
                    (Err(_), true) => None,
                };
                healthy &= ready == Some(true);
                roles.push(RoleStatus {
                    role: role.name(),
                    unit,
                    state: Some(state),
                    ready,
                    error: None,
                });
            }
            Err(error) => {
                healthy = false;
                roles.push(RoleStatus {
                    role: role.name(),
                    unit,
                    state: None,
                    ready: None,
                    error: Some(format!("{error:#}")),
                });
            }
        }
    }
    print_json(&serde_json::json!({
        "healthy": healthy, "config_error": config_error, "roles": roles,
    }))?;
    if healthy { Ok(()) } else { anyhow::bail!("部署未完全就绪") }
}
