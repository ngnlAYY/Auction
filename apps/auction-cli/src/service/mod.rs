//! 分体 systemd 部署的顺序编排：严格预检 → 迁移 → Worker/Admin/Public 逐个启动并等待
//! readiness；任一步失败只回滚本次启动的 unit，不回退已提交的迁移。停止顺序相反。

use std::{
    net::TcpListener,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    time::Duration,
};

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::app::{print_json, progress};
use crate::cli::{ServiceAction, ServiceArgs, UnitArgs};
use crate::deployment::receipt::{
    self, Action, Artifact, DeploymentReceipt, MigrationRecord, Outcome, ReceiptContext,
};
use crate::deployment::{ConfigDir, Deployment, Endpoint};

mod guard;
mod probe;
mod shutdown;
mod systemd;

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

/// 成功（含 `already_running`）回执先于成功输出发布；写不进去就整体失败。
#[allow(clippy::too_many_arguments)]
async fn publish_receipt(
    units: &UnitArgs,
    systemctl: &Systemctl,
    action: Action,
    outcome: Outcome,
    started_at: DateTime<Utc>,
    context: &ReceiptContext,
    migration: Option<MigrationRecord>,
    failure: Option<String>,
) -> anyhow::Result<DeploymentReceipt> {
    let artifacts = collect_artifacts(systemctl).await;
    receipt::record(
        &units.state_dir,
        action,
        outcome,
        started_at,
        context,
        migration,
        artifacts,
        failure,
    )
    .context("写入部署回执失败")
}

/// 失败回执尽力追加；回执本身写不进去时把它附在原错误上，不掩盖部署失败原因。
async fn record_failure(
    units: &UnitArgs,
    systemctl: &Systemctl,
    action: Action,
    started_at: DateTime<Utc>,
    context: &ReceiptContext,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    let failure = receipt::failure_text(error);
    if let Err(record_error) = publish_receipt(
        units,
        systemctl,
        action,
        Outcome::Failed,
        started_at,
        context,
        None,
        Some(failure),
    )
    .await
    {
        return Err(anyhow::anyhow!("{error:#}；部署回执也未写入：{record_error:#}"));
    }
    Ok(())
}

/// 只把实际运行的三个角色进程作为程序证据；停止或不可读的角色不伪造摘要。
async fn collect_artifacts(systemctl: &Systemctl) -> Vec<Artifact> {
    let mut artifacts = Vec::with_capacity(3);
    for role in ManagedRole::START_ORDER {
        let Ok(state) = systemctl.show(role).await else { continue };
        if state.running() {
            artifacts.push(receipt::artifact_from_process(role.name(), state.main_pid));
        }
    }
    artifacts
}

fn receipt_summary(units: &UnitArgs, receipt: &DeploymentReceipt) -> serde_json::Value {
    serde_json::json!({
        "receipt_id": receipt.receipt_id,
        "path": receipt::receipts_dir(&units.state_dir)
            .join(format!("{}.json", receipt.receipt_id)),
        "outcome": receipt.outcome,
    })
}

/// 状态查询不取部署锁、不要求严格权限；配置无法加载时仍报告 systemd 状态。
async fn status(config_dir: &ConfigDir, systemctl: &Systemctl) -> anyhow::Result<()> {
    let deployment = crate::deployment::check(config_dir, false);
    let config_error = deployment.as_ref().err().map(|error| format!("{error:#}"));
    let mut roles = Vec::with_capacity(3);
    let mut healthy = true;
    for role in ManagedRole::START_ORDER {
        let unit = systemctl.unit(role).to_owned();
        match systemctl.show(role).await {
            Ok(state) => {
                let ready = match (&deployment, state.running()) {
                    (Ok(deployment), true) => match endpoint(deployment, role) {
                        Ok((endpoint, path)) => Some(probe::ready(&endpoint, path).await),
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

/// 严格预检并核对 unit 装载状态、配置绑定与 Admin runtime 文件属主。
pub(crate) async fn prepare(
    config_dir: &ConfigDir,
    systemctl: &Systemctl,
) -> anyhow::Result<Deployment> {
    if guard::managed_lock(config_dir)?.is_some() {
        anyhow::bail!("该配置目录属于合并部署（auction-server）；service 命令只用于分体部署");
    }
    let deployment = crate::deployment::check(config_dir, true).context("部署配置严格预检失败")?;
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
    shutdown_gate: Option<serde_json::Value>,
}

impl LaunchReport {
    fn report_json(&self, units: &UnitArgs, receipt: &DeploymentReceipt) -> serde_json::Value {
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

async fn rollback(
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

async fn wait_ready(
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
            if state.running() && probe::ready(&endpoint, path).await {
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
async fn observe(
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

fn endpoint(
    deployment: &Deployment,
    role: ManagedRole,
) -> anyhow::Result<(Endpoint, &'static str)> {
    Ok(match role {
        ManagedRole::Worker => (
            deployment.worker_control.clone().context("Worker control 未启用")?,
            probe::CONTROL_READY_PATH,
        ),
        ManagedRole::Admin => (deployment.admin_endpoint.clone(), probe::HTTP_READY_PATH),
        ManagedRole::Public => (deployment.public_endpoint.clone(), probe::HTTP_READY_PATH),
    })
}

const fn config_role(role: ManagedRole) -> auction_config::Role {
    match role {
        ManagedRole::Worker => auction_config::Role::Worker,
        ManagedRole::Admin => auction_config::Role::Admin,
        ManagedRole::Public => auction_config::Role::Public,
    }
}

fn load_owner(
    owner: &crate::cli::OwnerSecretsArg,
) -> anyhow::Result<auction_config::PostgresOwnerSecrets> {
    crate::database::load_owner_secrets(&owner.owner_secrets)
}

async fn migrate(
    deployment: &Deployment,
    owner: &auction_config::PostgresOwnerSecrets,
) -> anyhow::Result<MigrationOutcome> {
    let report = crate::database::apply_migrations(&deployment.worker, owner).await?;
    Ok(MigrationOutcome {
        record: MigrationRecord {
            before_version: report.before_version,
            after_version: report.after_version,
            expected_version: report.expected_version,
            applied: report.applied,
        },
        report: serde_json::to_value(&report)?,
    })
}

/// Admin 以原子替换保存 runtime：文件与可写父目录必须归 Admin unit 的固定服务账号。
fn require_runtime_owner(path: &Path, state: &UnitState) -> anyhow::Result<()> {
    if state.dynamic_user {
        anyhow::bail!("Admin runtime 需要固定服务账号；请关闭 Admin unit 的 DynamicUser");
    }
    let uid = service_uid(&state.user)?;
    let file = std::fs::symlink_metadata(path).context("读取 Admin runtime 文件失败")?;
    let parent = path.parent().context("Admin runtime 缺少父目录")?;
    let directory = std::fs::symlink_metadata(parent).context("读取 Admin runtime 父目录失败")?;
    if file.uid() != uid
        || directory.uid() != uid
        || directory.permissions().mode() & 0o300 != 0o300
    {
        anyhow::bail!(
            "Admin runtime 文件与父目录必须归 Admin unit 账号（UID {uid}）所有且父目录可写：{}",
            path.display()
        );
    }
    Ok(())
}

/// systemd 的空 User 表示 root；用户名通过 NSS 解析，不用调用者 UID 推断。
pub(crate) fn service_uid(user: &str) -> anyhow::Result<u32> {
    if user.is_empty() {
        return Ok(0);
    }
    if let Ok(uid) = user.parse::<u32>() {
        return Ok(uid);
    }
    let name = std::ffi::CString::new(user).context("unit User 含非法字符")?;
    let mut buffer = vec![0_u8; 4096];
    loop {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY：name 以 NUL 结尾；record、buffer、result 在调用期间有效且互不重叠。
        let status = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || result.is_null() {
            anyhow::bail!("无法解析 unit User：{user}");
        }
        // SAFETY：getpwnam_r 成功且 result 非空，record 已完整初始化。
        return Ok(unsafe { record.assume_init().pw_uid });
    }
}
