//! `auction-cli rollback`：换回 `deploy` 保留的旧程序。
//!
//! 只允许数据库没有新增迁移、或新增迁移全部登记为 compatible 的区间；含 breaking 迁移的回退
//! 必须按 `ops/deploy/RECOVERY.md` 恢复升级前备份并做外部历史核对。回退同样先过时段与在途
//! 发送门禁，并把当前程序保留成新的回滚记录，使回退本身可逆。

use std::{path::Path, time::Duration};

use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

use crate::app::{print_json, progress};
use crate::cli::RollbackArgs;
use crate::deployment::ConfigDir;
use crate::deployment::receipt::{self, Action, DeploymentReceipt, Outcome, ReceiptContext};
use crate::service::{self, ManagedRole, Systemctl};

use super::install::{self, RollbackRecord};
use super::{
    artifact, gates, record_failure, role_binary, running_roles, shared_service_uid, unit_states,
};

pub(crate) async fn run(config_dir: &ConfigDir, args: RollbackArgs) -> anyhow::Result<()> {
    let RollbackArgs {
        receipt_id,
        units,
        shutdown,
        receipt: receipt_args,
        install_dir: explicit_install_dir,
        send_gate_timeout_seconds,
        startup_timeout_seconds,
        dry_run,
    } = args;
    if !cfg!(feature = "postgres") {
        anyhow::bail!("rollback 需要只读门禁；当前 auction-cli 未编译 postgres feature");
    }
    let record_id = match receipt_id {
        Some(value) => Uuid::parse_str(&value).context("--receipt-id 必须是回滚记录的 UUID")?,
        None => install::latest(&units.state_dir)?.0.record_id,
    };
    let (record, record_directory) = install::load(&units.state_dir, record_id)?;
    let context = ReceiptContext::resolve(None, receipt_args.actor.as_deref())?;
    let started_at = Utc::now();
    let systemctl = Systemctl::new(&units)?;
    let _lock = service::deployment_lock(&units)?;
    let deployment = service::prepare(config_dir, &systemctl).await?;
    let states = unit_states(&systemctl).await?;
    let install_dir =
        gates::resolve_install_dir(explicit_install_dir.as_deref(), &running_roles(&states))?;
    if install_dir.display().to_string() != record.install_directory {
        anyhow::bail!(
            "回滚记录的安装目录（{}）与当前安装目录（{}）不一致",
            record.install_directory,
            install_dir.display()
        );
    }
    install::require_install_dir(&install_dir)?;
    let service_uid = shared_service_uid(&states)?;
    let deployed_receipt = receipt::read(&units.state_dir, record.deploy_receipt_id)
        .context("读取回滚记录对应的部署回执失败")?;
    if deployed_receipt.outcome != Outcome::Succeeded {
        anyhow::bail!("回滚记录对应的部署回执不是成功状态");
    }
    let deployed = deployed_receipt.deploy.clone().context("回滚记录对应的部署回执缺少制品证据")?;
    require_installed_matches(&deployed_receipt, &deployed.cli_sha256, &install_dir)?;

    let gate =
        service::shutdown_gate(config_dir, Some(&deployment.worker), "rollback", &shutdown).await?;
    let current_version = gates::database_version(&deployment.worker).await?;
    let suffix =
        gates::require_compatible_suffix(&deployment.worker, record.previous_expected_version)
            .await?;
    let in_flight = gates::require_no_in_flight_sends(
        &deployment.worker,
        Duration::from_secs(send_gate_timeout_seconds),
    )
    .await?;
    let mut report = serde_json::json!({
        "action": "rollback",
        "rollback_record": install::record_projection(&record),
        "install_directory": install_dir,
        "gates": {
            "shutdown": gate,
            "versions": {
                "database": current_version,
                "target_expected_version": record.previous_expected_version,
                "suffix": suffix,
            },
            "in_flight_sends": in_flight,
        },
    });
    if dry_run {
        report["outcome"] = "planned".into();
        report["dry_run"] = true.into();
        report["planned_steps"] = serde_json::json!([
            "按 Public → Admin → Worker 停止三角色",
            "再次保留当前程序，使本次回退可逆",
            "从回滚记录原子安装旧程序（不执行迁移）",
            "按 Worker → Admin → Public 启动并等待 readiness",
            "复核运行程序摘要与回滚记录一致",
            "追加回退回执",
        ]);
        return print_json(&report);
    }

    match apply(
        &units,
        &systemctl,
        &deployment,
        &record,
        &record_directory,
        &install_dir,
        service_uid,
        Duration::from_secs(startup_timeout_seconds),
        &gate,
        &context,
        started_at,
        deployed,
    )
    .await
    {
        Ok(value) => {
            report["outcome"] = "started".into();
            report["rollback"] = value;
            print_json(&report)
        }
        Err(error) => {
            record_failure(&units, &systemctl, Action::Rollback, started_at, &context, &error)
                .await?;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply(
    units: &crate::cli::UnitArgs,
    systemctl: &Systemctl,
    deployment: &crate::deployment::Deployment,
    record: &RollbackRecord,
    record_directory: &Path,
    install_dir: &Path,
    service_uid: u32,
    startup_timeout: Duration,
    gate: &service::ShutdownGateReport<'_>,
    context: &ReceiptContext,
    started_at: chrono::DateTime<Utc>,
    deployed: crate::deployment::receipt::DeployRecord,
) -> anyhow::Result<serde_json::Value> {
    progress("停止三角色（Public → Admin → Worker）");
    service::stop_all(systemctl).await?;
    progress("保留当前程序以便再次前进");
    let reverse = install::retain(
        &units.state_dir,
        Uuid::new_v4(),
        install_dir,
        record.deployed_expected_version,
        record.previous_expected_version,
        Some(&deployed.source_commit),
    )
    .context("保留当前程序失败")?;
    progress("安装回滚记录中的旧程序");
    install::install_retained(record, record_directory, install_dir, service_uid)?;
    let launch = match service::launch(
        systemctl,
        deployment,
        None,
        startup_timeout,
        Some(serde_json::to_value(gate)?),
    )
    .await
    {
        Ok(launch) => launch,
        Err(error) => {
            return Err(error.context(format!(
                "旧程序已安装但启动失败；服务当前停止。可用 \
                 auction-cli rollback --receipt-id {} 重新前进，或按 ops/deploy/RECOVERY.md 恢复",
                reverse.record_id
            )));
        }
    };
    let artifacts = verify_running(systemctl, record).await?;
    let mut deploy = deployed;
    deploy.rollback_record_id = Some(reverse.record_id);
    deploy.migration_drill_id = None;
    deploy.backup_archive = None;
    deploy.backup_manifest = None;
    let published = receipt::record_deploy(
        &units.state_dir,
        Action::Rollback,
        Outcome::Succeeded,
        started_at,
        context,
        None,
        artifacts,
        deploy,
        None,
    )
    .context("写入回退回执失败")?;
    Ok(serde_json::json!({
        "reversed_rollback_record": install::record_projection(&reverse),
        "roles": launch.roles,
        "deployment_receipt": super::receipt_summary(units, &published),
    }))
}

/// 安装目录中的程序必须仍是该回滚记录对应的部署版本；否则回退目标不明确。
fn require_installed_matches(
    deployed: &DeploymentReceipt,
    cli_sha256: &str,
    install_dir: &Path,
) -> anyhow::Result<()> {
    let cli = install_dir.join("auction-cli");
    let (sha256, _) = artifact::sha256_path(&cli).context("读取安装目录的 auction-cli 失败")?;
    if sha256 != cli_sha256 {
        anyhow::bail!("安装目录的 auction-cli 不是该回滚记录对应的部署版本");
    }
    for (role, name) in [
        (ManagedRole::Worker, "auction-worker"),
        (ManagedRole::Admin, "auction-admin-api"),
        (ManagedRole::Public, "auction-public-api"),
    ] {
        let recorded = deployed
            .artifacts
            .iter()
            .find(|artifact| artifact.role == role.name())
            .and_then(|artifact| artifact.sha256.as_deref())
            .with_context(|| format!("部署回执缺少 {} 的程序摘要", role.name()))?;
        let (sha256, _) = artifact::sha256_path(&install_dir.join(name))
            .with_context(|| format!("读取安装目录的 {name} 失败"))?;
        if sha256 != recorded {
            anyhow::bail!("安装目录的 {name} 不是该回滚记录对应的部署版本");
        }
    }
    Ok(())
}

async fn verify_running(
    systemctl: &Systemctl,
    record: &RollbackRecord,
) -> anyhow::Result<Vec<receipt::Artifact>> {
    let mut artifacts = Vec::with_capacity(ManagedRole::START_ORDER.len());
    for role in ManagedRole::START_ORDER {
        let state = systemctl.show(role).await?;
        if !state.running() {
            anyhow::bail!("{} 未运行，无法核对回退后的程序摘要", role.name());
        }
        let artifact = receipt::artifact_from_process(role.name(), state.main_pid);
        let expected = record
            .binary(role_binary(role))
            .with_context(|| format!("回滚记录缺少 {} 程序", role.name()))?;
        if artifact.sha256.as_deref() != Some(expected.sha256.as_str()) {
            anyhow::bail!("{} 运行的程序摘要与回滚记录不一致", role.name());
        }
        artifacts.push(artifact);
    }
    Ok(artifacts)
}
