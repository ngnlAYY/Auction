//! `auction-cli rollback`：把三个运行角色换回 `deploy` 保留的旧程序，安装目录中的 auction-cli
//! 保持不变（它要继续管理可能已超前的数据库，见 `preconditions`）。
//!
//! 只允许数据库没有新增迁移、或新增迁移全部登记为 compatible 的区间；跨迁移回退还要求目标角色
//! 的内嵌 schema 已按摘要绑定（未知时只允许同版本）、三角色有效配置显式为 compatible 且目标程序
//! 支持该判定。含 breaking 迁移的回退必须按
//! `ops/deploy/RECOVERY.md` 恢复升级前备份并做外部历史核对。回退同样先过安装目录信任、时段、
//! 磁盘空间与在途发送门禁，并把当前三个角色程序保留成新的回滚记录，使回退本身可逆。
//!
//! 先取得部署锁并固定状态目录，再经同一个描述符读取回滚记录、部署回执与保留程序：持锁之前或
//! 之后按路径读取，都可能读到被改名替换的另一棵树。

use std::{path::Path, time::Duration};

use anyhow::Context;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::install::{self, ROLE_BINARIES, RecordDirectory, RetainRequest, RollbackRecord};
use super::receipts::{FailedReceipt, Failure, settle};
use super::verification::{install_dir_artifacts, run_accounts};
use super::{gates, role_binary, running_roles, space, unit_states};
use crate::app::{print_json, progress};
use crate::cli::{RollbackArgs, UnitArgs};
use crate::config_dir::{ConfigDir, Deployment};
use crate::receipt::{self, Action, DeployRecord, DeploymentReceipt, Outcome, ReceiptContext};
use crate::service::{self, ManagedRole, Systemctl};
use crate::support::digest::sha256_path;
use crate::support::files::PrivateDirectory;

mod guidance;
mod identity;
mod preconditions;

use self::guidance::{install_failed, stopped_before_install};

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
    let receipt_id = receipt_id
        .map(|value| Uuid::parse_str(&value).context("--receipt-id 必须是回滚记录的 UUID"))
        .transpose()?;
    let context = ReceiptContext::resolve(None, actor.actor.as_deref())?;
    let started_at = Utc::now();
    let systemctl = Systemctl::new(&units)?;
    let lock = service::deployment_lock(&units)?;
    let state = lock.directory();
    // latest 已对选中的记录完成同样的摘要复核，不再重复散列保留程序。
    let (record, record_directory) = match receipt_id {
        Some(value) => install::load(state, value)?,
        None => install::latest(state)?,
    };
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
    install::require_install_dir(&install_dir, &run_accounts(&states)?)?;
    let deployed_receipt =
        receipt::read(state, record.deploy_receipt_id).context("读取回滚记录对应的部署回执失败")?;
    preconditions::require_rollbackable_receipt(&deployed_receipt)?;
    let installed = preconditions::require_installed_matches(&deployed_receipt, &install_dir)?;
    preconditions::require_target_differs(&record, &installed)?;
    let kept_cli_sha256 = preconditions::require_kept_cli(&install_dir)?;

    let gate =
        service::shutdown_gate(config_dir, Some(&deployment.worker), "rollback", &shutdown).await?;
    let current_version = gates::database_version(&deployment.worker).await?;
    let cli_schema = auction_storage::postgres_expected_migration_version();
    preconditions::require_cli_matches_database(cli_schema, current_version)?;
    let (target_schema, target_schema_source) = record.target_schema();
    preconditions::require_known_schema_across_migrations(
        target_schema,
        target_schema_source,
        current_version,
    )?;
    let suffix = gates::require_compatible_suffix(&deployment.worker, target_schema).await?;
    let readiness = preconditions::require_compatible_readiness(
        &deployment.migration_compatibility_modes,
        target_schema,
        current_version,
    )?;
    // 停机后才写入的反向回滚记录（只保留三个角色）与角色程序安装临时文件：空间不足必须在停机
    // 前发现。余量只按实际写入/替换字节加固定 64 MiB，不套定时备份的比例下限。
    let incoming = ROLE_BINARIES
        .into_iter()
        .map(|name| {
            record.binary(name).map(|binary| (name, binary.bytes)).context("回滚记录缺少角色程序")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let disk_space = space::require_space(
        None,
        &units.state_dir,
        &install_dir,
        &incoming,
        space::SpacePolicy::Rollback,
    )
    .context("磁盘空间门禁失败")?;
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
                "target_role_schema_version": target_schema,
                "target_role_schema_source": target_schema_source.as_str(),
                "kept_cli_schema_version": cli_schema,
                "suffix": suffix,
            },
            "readiness": readiness,
            "disk_space": disk_space,
            "in_flight_sends": in_flight,
        },
    });
    if dry_run {
        report["outcome"] = "planned".into();
        report["dry_run"] = true.into();
        report["planned_steps"] = serde_json::json!([
            "按 Public → Admin → Worker 停止三角色",
            "把当前三个角色程序保留为新的回滚记录，使本次回退可逆",
            "从回滚记录逐个原子替换三个角色程序（root:root 0755；整体不是原子的，中途失败时从反向回滚记录换回已替换的程序、不启动；不执行迁移），安装目录中的 auction-cli 保持不变",
            "按 Worker → Admin → Public 启动并等待 readiness",
            "复核运行程序摘要与回滚记录一致、auction-cli 未变",
            "追加回退回执",
        ]);
        return print_json(&report);
    }

    // 停机前最后一道门：锁、回执与回滚记录必须仍属于持锁时固定的状态目录。
    lock.verify_unchanged()?;
    let shutdown_gate = serde_json::to_value(&gate)?;
    // 反向记录与其回退回执共享同一标识，和 deploy 一致，回退本身也能被再次读回；回退回执的
    // 部署证据只由目标组件的事实构造，停机前完成，停机后的任何失败都能以同一标识发布。
    let reverse_receipt_id = Uuid::new_v4();
    let cli = identity::kept_cli_identity(&deployed_receipt, &kept_cli_sha256, cli_schema);
    let reverse_deploy = identity::rollback_deploy_record(
        &record,
        &install_dir,
        current_version,
        cli,
        reverse_receipt_id,
    );
    let plan = RollbackPlan {
        units: &units,
        state,
        systemctl: &systemctl,
        deployment: &deployment,
        record: &record,
        record_directory: &record_directory,
        install_dir: &install_dir,
        database_version: current_version,
        startup_timeout: Duration::from_secs(startup_timeout_seconds),
        context: &context,
        started_at,
        deployed_receipt: &deployed_receipt,
        kept_cli_sha256: &kept_cli_sha256,
        reverse_receipt_id,
        deploy: &reverse_deploy,
    };
    match apply(&plan, shutdown_gate).await {
        Ok(value) => {
            report["outcome"] = "started".into();
            report["rollback"] = value;
            print_json(&report)
        }
        Err(failure) => {
            Err(settle(state, &systemctl, Action::Rollback, started_at, &context, failure).await)
        }
    }
}

/// 停机前已核对完毕的回退输入；停机后的步骤只读取它，不再做新的门禁判定。
struct RollbackPlan<'a> {
    units: &'a UnitArgs,
    /// 持锁时固定的部署状态目录；反向记录与回执都写在它的描述符下。
    state: &'a PrivateDirectory,
    systemctl: &'a Systemctl,
    deployment: &'a Deployment,
    record: &'a RollbackRecord,
    record_directory: &'a RecordDirectory,
    install_dir: &'a Path,
    /// 停机前门禁读到的数据库迁移版本；回退不执行迁移，停机后的失败仍以它描述恢复路径。
    database_version: i64,
    startup_timeout: Duration,
    context: &'a ReceiptContext,
    started_at: DateTime<Utc>,
    /// 回滚记录对应的部署回执；它描述当前安装的三个角色程序。
    deployed_receipt: &'a DeploymentReceipt,
    /// 停机前读到的安装目录 auction-cli 摘要；回退前后必须不变。
    kept_cli_sha256: &'a str,
    reverse_receipt_id: Uuid,
    deploy: &'a DeployRecord,
}

/// 停机后的回退主体。反向记录写出之后的失败都以反向记录的标识发布一条失败回执，
/// `rollback --receipt-id <反向记录>` 因此总能读回实际安装的版本。
async fn apply(
    plan: &RollbackPlan<'_>,
    shutdown_gate: serde_json::Value,
) -> Result<serde_json::Value, Failure> {
    let RollbackPlan { units, state, systemctl, record, install_dir, reverse_receipt_id, .. } =
        *plan;
    progress("停止三角色（Public → Admin → Worker）");
    service::stop_all(systemctl).await.map_err(Failure::unrecorded)?;
    progress("保留当前三个角色程序以便再次前进");
    let request = RetainRequest {
        deploy_receipt_id: reverse_receipt_id,
        install_dir,
        programs: &ROLE_BINARIES,
        observed_database_version: plan.database_version,
        installed_role_schema_version: record.retained_roles.schema_version,
    };
    // 反向记录的身份按保留副本的摘要从成功回执重新绑定；部署回执失败时它只描述尝试安装的目标。
    let (reverse, reverse_directory) = install::retain(state, &request, |binaries| {
        identity::reverse_identity(state, plan.deployed_receipt, record, binaries)
    })
    .context("保留当前程序失败")
    .map_err(|error| {
        let reverse_directory = install::record_dir(&state.path, reverse_receipt_id);
        stopped_before_install(
            error,
            install_dir,
            plan.database_version,
            record,
            &reverse_directory,
        )
    })?;
    let failed = FailedReceipt {
        state,
        receipt_id: reverse_receipt_id,
        action: Action::Rollback,
        started_at: plan.started_at,
        context: plan.context,
        deploy: plan.deploy,
    };
    progress("安装回滚记录中的三个角色程序（auction-cli 保持不变）");
    if let Err(failure) = install::install_retained(
        record,
        plan.record_directory,
        install_dir,
        &reverse,
        &reverse_directory,
    ) {
        let failure = install_failed(
            failure,
            install_dir,
            plan.database_version,
            record,
            reverse_directory.path(),
        );
        // 失败回执记录安装目录实际文件的摘要（已换回时是回退前的程序，否则可能新旧混合），
        // 不冒充回滚记录的摘要；本次调用只发布这一条。
        return Err(failed.publish(failure, None, Ok(install_dir_artifacts(install_dir))));
    }
    if let Err(error) = require_cli_unchanged(install_dir, plan.kept_cli_sha256) {
        let failure = error.context(format!(
            "三个角色程序已换回但安装目录的 auction-cli 在回退期间被改变，未启动服务；人工核对后可用 \
             auction-cli rollback --receipt-id {} 重新前进，或按 ops/deploy/RECOVERY.md 恢复",
            reverse.record_id
        ));
        return Err(failed.publish(
            failure,
            None,
            installed_retained_artifacts(record, install_dir),
        ));
    }
    let launch = match service::launch(
        systemctl,
        plan.deployment,
        None,
        plan.startup_timeout,
        Some(shutdown_gate),
    )
    .await
    {
        Ok(launch) => launch,
        Err(error) => {
            let failure = error.context(format!(
                "旧角色程序已安装但启动失败；服务当前停止。可用 auction-cli rollback \
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
            let failure = error.context(format!(
                "旧角色程序已启动但运行程序核对失败；可用 auction-cli rollback --receipt-id {} \
                 重新前进，或按 ops/deploy/RECOVERY.md 恢复",
                reverse.record_id
            ));
            let artifacts = installed_retained_artifacts(record, install_dir);
            return Err(failed.publish(failure, None, artifacts));
        }
    };
    let published = match receipt::record_deploy(
        state,
        reverse_receipt_id,
        Action::Rollback,
        Outcome::Succeeded,
        plan.started_at,
        plan.context,
        None,
        artifacts.clone(),
        plan.deploy.clone(),
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

/// rollback 从不替换 auction-cli：换回角色程序之后它的摘要必须与停机前相同。
fn require_cli_unchanged(install_dir: &Path, expected_sha256: &str) -> anyhow::Result<()> {
    let (sha256, _) =
        sha256_path(&install_dir.join("auction-cli")).context("读取安装目录的 auction-cli 失败")?;
    if sha256 != expected_sha256 {
        anyhow::bail!("安装目录的 auction-cli 摘要与停机前不一致");
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

#[cfg(test)]
mod tests;
