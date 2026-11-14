//! 角色的准备、启动、就绪等待、回滚与停止。

use std::net::TcpListener;
use std::time::Duration;

use anyhow::Context;
use auction_process::{CONTROL_READY_PATH, HTTP_READY_PATH, probe_ready};

use super::migration::{migrate, require_runtime_owner};
use super::receipts::receipt_summary;
use super::systemd::{ManagedRole, Systemctl, UnitState};
use super::{POLL_INTERVAL, guard, systemd};
use crate::app::progress;
use crate::cli::UnitArgs;
use crate::config_dir::{ConfigDir, Deployment, Endpoint};
use crate::receipt::{DeploymentReceipt, MigrationRecord};

/// 严格预检并核对 unit 装载状态、配置绑定与 Admin runtime 文件属主。
pub(crate) async fn prepare(
    config_dir: &ConfigDir,
    systemctl: &Systemctl,
) -> anyhow::Result<Deployment> {
    if guard::managed_lock(config_dir)?.is_some() {
        anyhow::bail!("该配置目录属于合并部署（auction-server）；service 命令只用于分体部署");
    }
    let deployment = crate::config_dir::check(config_dir, true).context("部署配置严格预检失败")?;
    if deployment.worker_control.is_none() {
        anyhow::bail!("service 编排需要启用 [worker.control] 以探测 Worker readiness");
    }
    for role in ManagedRole::START_ORDER {
        let state = systemctl.show(role).await?;
        if state.load_state != "loaded" {
            anyhow::bail!("{} unit 未装载：{}", role.name(), systemctl.unit(role));
        }
        systemd::require_config_binding(role, &state, &config_dir.role_path(config_role(role)))?;
        if role == ManagedRole::Admin
            && let Some(runtime) = deployment.admin.runtime_config_path.as_deref()
        {
            require_runtime_owner(runtime, &state)?;
        }
    }
    Ok(deployment)
}

/// 顺序启动三个角色；`migrate` 为 false 时只启动（回退路径不得让旧程序重放迁移）。
pub(crate) async fn launch(
    systemctl: &Systemctl,
    deployment: &Deployment,
    owner: Option<&auction_config::PostgresOwnerSecrets>,
    timeout: Duration,
    shutdown_gate: Option<serde_json::Value>,
) -> anyhow::Result<LaunchReport> {
    for role in [ManagedRole::Admin, ManagedRole::Public] {
        if let Ok((Endpoint::Tcp(address), _)) = endpoint(deployment, role) {
            TcpListener::bind(address)
                .with_context(|| format!("{} 监听地址已被占用：{address}", role.name()))?;
        }
    }
    let migration = match owner {
        Some(owner) => Some(migrate(deployment, owner).await?),
        None => None,
    };
    let mut started = Vec::new();
    let mut roles = Vec::with_capacity(3);
    for role in ManagedRole::START_ORDER {
        progress(format_args!("启动 {}（{}）", role.name(), systemctl.unit(role)));
        // systemctl 失败时 job 可能已入队，因此先纳入回滚集合。
        started.push(role);
        let result = match systemctl.start(role).await {
            Ok(()) => wait_ready(systemctl, deployment, role, timeout).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(state) => roles.push(serde_json::json!({
                "role": role.name(), "unit": systemctl.unit(role), "main_pid": state.main_pid,
            })),
            Err(error) => return Err(rollback(systemctl, &started, error).await),
        }
    }
    Ok(LaunchReport { migration, roles, shutdown_gate })
}

/// 启动结果；迁移同时保留 stdout 投影与回执所需的窄记录。
pub(crate) struct LaunchReport {
    pub(crate) migration: Option<MigrationOutcome>,
    pub(crate) roles: Vec<serde_json::Value>,
    pub(super) shutdown_gate: Option<serde_json::Value>,
}

impl LaunchReport {
    pub(super) fn report_json(
        &self,
        units: &UnitArgs,
        receipt: &DeploymentReceipt,
    ) -> serde_json::Value {
        let mut report = serde_json::json!({
            "action": "start", "outcome": "started",
            "migration": self.migration.as_ref().map(|migration| &migration.report),
            "roles": self.roles,
            "deployment_receipt": receipt_summary(units, receipt),
        });
        if let Some(gate) = &self.shutdown_gate {
            report["shutdown_gate"] = gate.clone();
        }
        report
    }
}

pub(crate) struct MigrationOutcome {
    pub(crate) record: MigrationRecord,
    pub(crate) report: serde_json::Value,
}

pub(super) async fn rollback(
    systemctl: &Systemctl,
    started: &[ManagedRole],
    error: anyhow::Error,
) -> anyhow::Error {
    let mut failures = Vec::new();
    for role in started.iter().rev() {
        progress(format_args!("回滚：停止 {}", role.name()));
        if let Err(stop_error) = systemctl.stop(*role).await {
            failures.push(format!("{}: {stop_error:#}", role.name()));
        }
    }
    if failures.is_empty() {
        error.context("启动失败，本次启动的 unit 已停止；已提交的迁移不会回退")
    } else {
        error.context(format!("启动失败且回滚不完整：{}", failures.join(" | ")))
    }
}

pub(super) async fn wait_ready(
    systemctl: &Systemctl,
    deployment: &Deployment,
    role: ManagedRole,
    timeout: Duration,
) -> anyhow::Result<UnitState> {
    let (endpoint, path) = endpoint(deployment, role)?;
    let deadline = tokio::time::Instant::now() + timeout;
    // 同一预算覆盖状态查询、readiness 请求、身份复核和轮询等待；慢查询不能越过截止后报成功。
    let observe = async {
        loop {
            let state = systemctl.show(role).await?;
            if state.active_state == "failed" {
                anyhow::bail!(
                    "{} 启动失败；请查看 systemctl status {} 与 journalctl",
                    role.name(),
                    systemctl.unit(role)
                );
            }
            if state.running() && probe_ready(endpoint.readiness_target(), path).await {
                // 探测前后进程身份必须一致，否则可能是重启后的另一个实例。
                let current = systemctl.show(role).await?;
                if current.running()
                    && current.main_pid == state.main_pid
                    && current.invocation_id == state.invocation_id
                {
                    return Ok(current);
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    };
    match tokio::time::timeout_at(deadline, observe).await {
        Ok(result) if tokio::time::Instant::now() < deadline => {
            let current = result?;
            progress(format_args!("{} 已就绪（MainPID {}）", role.name(), current.main_pid));
            Ok(current)
        }
        _ => anyhow::bail!(
            "{} 未在 {} 秒内通过 readiness；请查看 journalctl -u {}",
            role.name(),
            timeout.as_secs(),
            systemctl.unit(role)
        ),
    }
}

/// 三角色均已运行时只按顺序复核 readiness，不执行任何启停或迁移。
pub(super) async fn observe(
    systemctl: &Systemctl,
    deployment: &Deployment,
    timeout: Duration,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut roles = Vec::with_capacity(3);
    for role in ManagedRole::START_ORDER {
        let state = wait_ready(systemctl, deployment, role, timeout).await?;
        roles.push(serde_json::json!({
            "role": role.name(), "unit": systemctl.unit(role), "main_pid": state.main_pid,
        }));
    }
    Ok(roles)
}

pub(crate) async fn stop_all(systemctl: &Systemctl) -> anyhow::Result<()> {
    for role in ManagedRole::START_ORDER.into_iter().rev() {
        progress(format_args!("停止 {}（{}）", role.name(), systemctl.unit(role)));
        systemctl.stop(role).await?;
        let state = systemctl.show(role).await?;
        if !state.stopped() {
            anyhow::bail!(
                "{} 停止后仍处于 {}；停止后续角色前请人工核对",
                role.name(),
                state.active_state
            );
        }
    }
    Ok(())
}

pub(super) fn endpoint(
    deployment: &Deployment,
    role: ManagedRole,
) -> anyhow::Result<(Endpoint, &'static str)> {
    Ok(match role {
        ManagedRole::Worker => (
            deployment.worker_control.clone().context("Worker control 未启用")?,
            CONTROL_READY_PATH,
        ),
        ManagedRole::Admin => (deployment.admin_endpoint.clone(), HTTP_READY_PATH),
        ManagedRole::Public => (deployment.public_endpoint.clone(), HTTP_READY_PATH),
    })
}

pub(super) const fn config_role(role: ManagedRole) -> auction_config::Role {
    match role {
        ManagedRole::Worker => auction_config::Role::Worker,
        ManagedRole::Admin => auction_config::Role::Admin,
        ManagedRole::Public => auction_config::Role::Public,
    }
}
