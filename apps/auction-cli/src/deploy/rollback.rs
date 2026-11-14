//! `auction-cli rollback`：换回 `deploy` 保留的旧程序。
//!
//! 只允许数据库没有新增迁移、或新增迁移全部登记为 compatible 的区间；含 breaking 迁移的回退
//! 必须按 `ops/deploy/RECOVERY.md` 恢复升级前备份并做外部历史核对。回退同样先过时段与在途
//! 发送门禁，并把当前程序保留成新的回滚记录，使回退本身可逆。

use std::{path::Path, time::Duration};

use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

use super::install::{self, RollbackRecord};
use super::receipts::{FailedReceipt, Failure, settle};
use super::verification::install_dir_artifacts;
use super::{artifact, gates, role_binary, running_roles, shared_service_uid, unit_states};
use crate::app::{print_json, progress};
use crate::cli::RollbackArgs;
use crate::config_dir::ConfigDir;
use crate::receipt::{self, Action, DeployRecord, DeploymentReceipt, Outcome, ReceiptContext};
use crate::service::{self, ManagedRole, Systemctl};

pub(crate) async fn run(config_dir: &ConfigDir, args: RollbackArgs) -> anyhow::Result<()> {
    let RollbackArgs {
        receipt_id,
        units,
        shutdown,
        actor,
        install_dir: explicit_install_dir,
        send_gate_timeout_seconds,
        startup_timeout_seconds,
        dry_run,
    } = args;
    // latest 已对选中的记录完成同样的摘要复核，不再重复散列保留程序。
    let (record, record_directory) = match receipt_id {
        Some(value) => install::load(
            &units.state_dir,
            Uuid::parse_str(&value).context("--receipt-id 必须是回滚记录的 UUID")?,
        )?,
        None => install::latest(&units.state_dir)?,
    };
    let context = ReceiptContext::resolve(None, actor.actor.as_deref())?;
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
    // 启动失败也会发布同 ID 的失败部署回执：新程序已安装但未就绪，仍必须允许恢复旧程序。
    if !matches!(deployed_receipt.outcome, Outcome::Succeeded | Outcome::Failed) {
        anyhow::bail!("回滚记录对应的部署回执不是可回滚状态");
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

    let shutdown_gate = serde_json::to_value(&gate)?;
    // 反向记录与其回退回执共享同一标识，和 deploy 一致，回退本身也能被再次读回；反向回执的
    // 部署证据只由回滚记录推导，停机前构造，停机后的任何失败都能以同一标识发布。
    let reverse_receipt_id = Uuid::new_v4();
    let current_source_commit = deployed.source_commit.clone();
    let reverse_deploy = reverse_deploy_record(&record, deployed, reverse_receipt_id)?;
    match apply(
        &units,
        &systemctl,
        &deployment,
        &record,
        &record_directory,
        &install_dir,
        service_uid,
        Duration::from_secs(startup_timeout_seconds),
        shutdown_gate,
        &context,
        started_at,
        &current_source_commit,
        reverse_receipt_id,
        &reverse_deploy,
    )
    .await
    {
        Ok(value) => {
            report["outcome"] = "started".into();
            report["rollback"] = value;
            print_json(&report)
        }
        Err(failure) => {
            Err(settle(&units, &systemctl, Action::Rollback, started_at, &context, failure).await)
        }
    }
}

/// 停机后的回退主体。反向记录写出之后的失败都以反向记录的标识发布恰好一条失败回执，
/// `rollback --receipt-id <反向记录>` 因此总能读回实际安装的版本。
#[allow(clippy::too_many_arguments)]
async fn apply(
    units: &crate::cli::UnitArgs,
    systemctl: &Systemctl,
    deployment: &crate::config_dir::Deployment,
    record: &RollbackRecord,
    record_directory: &Path,
    install_dir: &Path,
    service_uid: u32,
    startup_timeout: Duration,
    shutdown_gate: serde_json::Value,
    context: &ReceiptContext,
    started_at: chrono::DateTime<Utc>,
    current_source_commit: &str,
    reverse_receipt_id: Uuid,
    deploy: &DeployRecord,
) -> Result<serde_json::Value, Failure> {
    progress("停止三角色（Public → Admin → Worker）");
    service::stop_all(systemctl).await.map_err(Failure::unrecorded)?;
    progress("保留当前程序以便再次前进");
    let reverse = install::retain(
        &units.state_dir,
        reverse_receipt_id,
        install_dir,
        record.deployed_expected_version,
        record.previous_expected_version,
        Some(current_source_commit),
    )
    .context("保留当前程序失败")
    .map_err(Failure::unrecorded)?;
    let failed = FailedReceipt {
        state_dir: &units.state_dir,
        receipt_id: reverse_receipt_id,
        action: Action::Rollback,
        started_at,
        context,
        deploy,
    };
    progress("安装回滚记录中的旧程序");
    if let Err(error) =
        install::install_retained(record, record_directory, install_dir, service_uid)
    {
        let failure = error.context(format!(
            "安装旧程序失败：三角色已停止；{} 中的程序可能已部分替换，回退前的程序完整保留在 \
             回滚记录 {}",
            install_dir.display(),
            reverse.record_id
        ));
        // 安装目录可能新旧混合：失败回执记录实际文件摘要，不冒充回滚记录的摘要。
        return Err(failed.publish(failure, None, Ok(install_dir_artifacts(install_dir))));
    }
    let launch =
        match service::launch(systemctl, deployment, None, startup_timeout, Some(shutdown_gate))
            .await
        {
            Ok(launch) => launch,
            Err(error) => {
                let failure = error.context(format!(
                    "旧程序已安装但启动失败；服务当前停止。可用 auction-cli rollback \
                     --receipt-id {} 重新前进，或按 ops/deploy/RECOVERY.md 恢复",
                    reverse.record_id
                ));
                // 没有可核对的运行进程：用保留记录的摘要描述已安装的旧程序。
                let artifacts = installed_retained_artifacts(record, install_dir);
                return Err(failed.publish(failure, None, artifacts));
            }
        };
    let artifacts = match verify_running(systemctl, record).await {
        Ok(artifacts) => artifacts,
        Err(error) => {
            let artifacts = installed_retained_artifacts(record, install_dir);
            return Err(failed.publish(error, None, artifacts));
        }
    };
    let published = match receipt::record_deploy(
        &units.state_dir,
        reverse_receipt_id,
        Action::Rollback,
        Outcome::Succeeded,
        started_at,
        context,
        None,
        artifacts.clone(),
        deploy.clone(),
        None,
    ) {
        Ok(published) => published,
        // 成功回执未写入时以同一标识补写失败回执；若成功回执其实已发布，无覆盖发布会拒绝
        // 补写，两段原因一并返回。
        Err(error) => {
            return Err(failed.publish(error.context("写入回退回执失败"), None, Ok(artifacts)));
        }
    };
    Ok(serde_json::json!({
        "reversed_rollback_record": install::record_projection(&reverse),
        "roles": launch.roles,
        "deployment_receipt": service::receipt_summary(units, &published),
    }))
}

/// 反向回执描述回滚后实际安装的版本：CLI 摘要与来源提交取自回滚记录；沿用回滚前版本的
/// 摘要会让再次前进时的安装目录核对失败。恢复点与演练只属于原部署，不带入反向回执。
fn reverse_deploy_record(
    record: &RollbackRecord,
    deployed: DeployRecord,
    reverse_receipt_id: Uuid,
) -> anyhow::Result<DeployRecord> {
    let mut deploy = deployed;
    deploy.cli_sha256 = restored_cli_sha256(record)?;
    if let Some(commit) = &record.previous_source_commit {
        deploy.source_commit = commit.clone();
    }
    deploy.rollback_record_id = Some(reverse_receipt_id);
    deploy.migration_drill_id = None;
    deploy.backup_archive = None;
    deploy.backup_manifest = None;
    Ok(deploy)
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

/// 回滚启动失败时没有可核对的运行进程；用保留记录的摘要描述已安装的旧程序。
fn installed_retained_artifacts(
    record: &RollbackRecord,
    install_dir: &Path,
) -> anyhow::Result<Vec<receipt::Artifact>> {
    let mut artifacts = Vec::with_capacity(ManagedRole::START_ORDER.len());
    for role in ManagedRole::START_ORDER {
        let name = role_binary(role);
        let binary = record.binary(name).with_context(|| format!("回滚记录缺少 {name} 程序"))?;
        artifacts.push(receipt::Artifact {
            role: role.name().to_owned(),
            path: Some(install_dir.join(name).display().to_string()),
            sha256: Some(binary.sha256.clone()),
            bytes: Some(binary.bytes),
        });
    }
    Ok(artifacts)
}

/// 反向回执必须记录实际安装的 CLI 摘要，不能沿用回滚前版本的摘要。
fn restored_cli_sha256(record: &RollbackRecord) -> anyhow::Result<String> {
    record
        .binaries
        .iter()
        .find(|binary| binary.name == "auction-cli")
        .map(|binary| binary.sha256.clone())
        .context("回滚记录缺少 auction-cli 摘要")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::Utc;
    use uuid::Uuid;

    use super::{installed_retained_artifacts, restored_cli_sha256, reverse_deploy_record};
    use crate::deploy::install::{FORMAT, RetainedBinary, RollbackRecord};
    use crate::receipt::DeployRecord;

    fn deployed(rollback_record_id: Uuid) -> DeployRecord {
        DeployRecord {
            artifact_directory: "/srv/artifact".to_owned(),
            package: "auction-rust-0.1.0-aaaaaaaaaaaa".to_owned(),
            source_commit: "f".repeat(40),
            source_fingerprint: "b".repeat(64),
            cli_sha256: "c".repeat(64),
            install_directory: "/opt/auction/bin".to_owned(),
            public_payload_sha256: "d".repeat(64),
            admin_payload_sha256: "e".repeat(64),
            migration_drill_id: Some("drill-1".to_owned()),
            backup_archive: Some("/var/backups/auction/a.dump".to_owned()),
            backup_manifest: Some("/var/backups/auction/a.dump.manifest".to_owned()),
            rollback_record_id: Some(rollback_record_id),
        }
    }

    fn record(cli_sha256: Option<&str>) -> RollbackRecord {
        let mut binaries = cli_sha256
            .map(|sha256| RetainedBinary {
                name: "auction-cli".to_owned(),
                sha256: sha256.to_owned(),
                bytes: 1,
            })
            .into_iter()
            .collect::<Vec<_>>();
        for name in ["auction-worker", "auction-admin-api", "auction-public-api"] {
            binaries.push(RetainedBinary {
                name: name.to_owned(),
                sha256: "c".repeat(64),
                bytes: 2,
            });
        }
        RollbackRecord {
            format: FORMAT.to_owned(),
            record_id: Uuid::new_v4(),
            deploy_receipt_id: Uuid::new_v4(),
            created_at: Utc::now(),
            install_directory: "/opt/auction/bin".to_owned(),
            previous_expected_version: 1,
            deployed_expected_version: 2,
            previous_source_commit: Some("a".repeat(40)),
            binaries,
        }
    }

    #[test]
    fn rollback_receipt_uses_retained_cli_digest() {
        let digest = "b".repeat(64);
        assert_eq!(restored_cli_sha256(&record(Some(&digest))).expect("CLI 摘要"), digest);
        assert!(restored_cli_sha256(&record(None)).is_err());
    }

    /// 反向回执在停机前由回滚记录推导：指向反向记录、换成旧 CLI 摘要与来源提交，
    /// 不沿用原部署的恢复点与演练；记录缺 CLI 摘要时在停机前失败。
    #[test]
    fn reverse_deploy_record_describes_the_restored_version() {
        let retained = record(Some(&"b".repeat(64)));
        let reverse_id = Uuid::new_v4();
        let reverse = reverse_deploy_record(&retained, deployed(retained.record_id), reverse_id)
            .expect("反向回执");
        assert_eq!(reverse.rollback_record_id, Some(reverse_id));
        assert_eq!(reverse.cli_sha256, "b".repeat(64));
        assert_eq!(Some(reverse.source_commit), retained.previous_source_commit);
        assert!(reverse.migration_drill_id.is_none());
        assert!(reverse.backup_archive.is_none() && reverse.backup_manifest.is_none());
        let incomplete = record(None);
        assert!(
            reverse_deploy_record(&incomplete, deployed(incomplete.record_id), reverse_id).is_err()
        );
    }

    #[test]
    fn failed_rollback_receipt_uses_installed_retained_artifacts() {
        let artifacts =
            installed_retained_artifacts(&record(Some(&"b".repeat(64))), Path::new("/opt/bin"))
                .expect("静态回执");
        assert_eq!(artifacts.len(), 3);
        assert!(artifacts.iter().all(|artifact| artifact.sha256.is_some()));
        assert!(artifacts.iter().all(|artifact| artifact.path.is_some()));
    }
}
