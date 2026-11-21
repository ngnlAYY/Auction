//! `auction-cli deploy <制品目录>`：分体部署的一条命令编排。
//!
//! 顺序：制品静态校验（文件白名单、权限、摘要、Web payload 标记）→ `--backup-dir` → 部署锁
//! （状态目录与各级祖先的属主、权限核对后以描述符固定）→ 严格配置、unit 绑定预检 → 安装目录
//! 信任与执行权限门禁（含部署执行账号能执行停机后要用的旧 CLI）→ 时段门禁 → 迁移版本与演练
//! 回执 → 磁盘空间门禁 → 在途发送门禁 → 复核状态目录未被替换 → 停机 → 停机备份并由新旧 CLI
//! 各校验一次 → 保留旧程序 → 逐个原子替换程序（root:root 0755）→ 迁移与顺序启动 → 运行程序摘要
//! 复核 → 部署回执。任一门禁失败都在停机前拒绝；停机后每次调用最多追加一条回执（写不进去时
//! 错误注明），安装后启动失败只停止本次启动的 unit，并把回退指引写入回执。

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use auction_time::Timestamp;

use crate::app::{print_json, progress};
use crate::cli::DeployArgs;
use crate::config_dir::ConfigDir;
use crate::receipt::{Action, ReceiptContext};
use crate::service::{self, Systemctl};
use crate::support::{digest::is_lower_hex, files::PrivateDirectory};

pub(crate) mod artifact;
mod drill;
mod execution;
mod gates;
mod install;
mod receipts;
mod rollback;
mod space;
mod verification;
use self::execution::{DeployPlan, deploy_locked};
use self::install::RetainedBinary;
use self::receipts::{artifact_projection, installed_identity, settle};
use self::verification::{
    install_dir_artifacts, require_artifact_cli, require_backup_dir, run_accounts,
};
pub(crate) use verification::{role_binary, running_roles, unit_states};

pub(crate) use rollback::run as run_rollback;

pub(crate) async fn run(config_dir: &ConfigDir, args: DeployArgs) -> anyhow::Result<()> {
    let artifact = artifact::inspect(&args.artifact_dir).context("部署制品校验失败")?;
    require_artifact_cli(&artifact)?;
    run_inspected(config_dir, &artifact, args).await
}

/// 制品已通过静态校验、当前程序也已核对为制品内 CLI 之后的编排。停机只发生在末尾的
/// [`deploy_locked`] 中，此前任一门禁失败都不停机、不安装、不写回执。
async fn run_inspected(
    config_dir: &ConfigDir,
    artifact: &artifact::Artifact,
    args: DeployArgs,
) -> anyhow::Result<()> {
    let DeployArgs {
        artifact_dir: _,
        units,
        owner,
        shutdown,
        actor,
        install_dir: explicit_install_dir,
        backup_dir,
        package_sha256,
        migration_drill_receipt,
        drill_max_age_days,
        send_gate_timeout_seconds,
        startup_timeout_seconds,
        dry_run,
    } = args;
    // 本地参数门禁先于任何锁、systemctl 与数据库访问，dry-run 同样执行。
    require_backup_dir(&backup_dir)?;
    let mut context = ReceiptContext::resolve(None, actor.actor.as_deref())?;
    let started_at = Timestamp::now();
    let owner = crate::database::load_owner_secrets(&owner.owner_secrets)?;
    let systemctl = Systemctl::new(&units)?;
    let state = crate::service::deployment_lock(&units)?;
    let deployment = service::prepare(config_dir, &systemctl).await?;
    let states = unit_states(&systemctl).await?;
    let running = running_roles(&states);
    let install_dir = gates::resolve_install_dir(explicit_install_dir.as_deref(), &running)?;
    install::require_install_dir(&install_dir, &run_accounts(&states)?)?;

    let gate =
        service::shutdown_gate(config_dir, Some(&deployment.worker), "deploy", &shutdown).await?;
    let database = gates::database_name(&deployment.worker)?;
    let expected_version = auction_storage::postgres_expected_migration_version();
    let database_facts = gates::database_facts(&deployment.worker).await?;
    let current_version = database_facts.version;
    if expected_version < current_version {
        anyhow::bail!(
            "制品内嵌 schema 版本 {expected_version} 低于数据库 {current_version}；\
             回退请使用 auction-cli rollback，含破坏性迁移只能按 ops/deploy/RECOVERY.md 恢复"
        );
    }
    // 观测到的数据库版本不等于旧角色程序的内嵌 schema：回退保留当前 CLI 后两者可以不同。旧角色
    // 的身份（含 schema）只按程序摘要从回执绑定，停机后保留时再以副本摘要绑定一次写入回滚记录。
    let (installed_roles, previous_package) =
        installed_roles_identity(state.directory(), &install_dir)?;
    let drill = if expected_version > current_version {
        let path = migration_drill_receipt.as_deref().with_context(|| {
            format!(
                "本次制品包含迁移（数据库 {current_version} → {expected_version}），\
                 必须提供 --migration-drill-receipt"
            )
        })?;
        let (previous_package, package_sha256) =
            drill_package_inputs(package_sha256.as_deref(), previous_package.as_deref())?;
        Some(
            drill::inspect(
                path,
                &drill::Requirements {
                    artifact,
                    current_version,
                    expected_version,
                    previous_package,
                    package_sha256,
                    protection_window_minutes: shutdown.protection_window_minutes,
                    max_age_days: drill_max_age_days,
                    now: Timestamp::now(),
                },
            )
            .await
            .context("迁移演练回执核对失败")?,
        )
    } else {
        None
    };
    // 停机备份、旧程序保留与安装临时文件都在停机后写入；空间不足必须在停机前发现。
    let incoming =
        artifact.binaries.iter().map(|binary| (binary.name, binary.bytes)).collect::<Vec<_>>();
    let disk_space = space::require_space(
        Some((&backup_dir, database_facts.size_bytes)),
        &units.state_dir,
        &install_dir,
        &incoming,
        space::SpacePolicy::Backup(&deployment.worker.backup),
    )
    .context("磁盘空间门禁失败")?;
    progress(format_args!("在途发送门禁：最多等待 {send_gate_timeout_seconds} 秒"));
    let in_flight = gates::require_no_in_flight_sends(
        &deployment.worker,
        Duration::from_secs(send_gate_timeout_seconds),
    )
    .await?;

    let versions = serde_json::json!({
        "database": database,
        "current_version": current_version,
        "expected_version": expected_version,
        "installed_roles": installed_roles,
    });
    let drill_report = drill.as_ref().map(|drill| {
        serde_json::json!({
            "drill_id": drill.drill_id,
            "before_version": drill.before_version,
            "after_version": drill.after_version,
            "minimum_window_minutes": drill.minimum_window_minutes,
            "date_utc": drill.date_utc.to_string(),
        })
    });
    let mut report = serde_json::json!({
        "action": "deploy",
        "artifact": artifact_projection(artifact),
        "install_directory": install_dir,
        "backup_directory": backup_dir,
        "gates": {
            "shutdown": gate,
            "versions": versions,
            "migration_drill": drill_report,
            "disk_space": disk_space,
            "in_flight_sends": in_flight,
        },
    });
    if dry_run {
        report["outcome"] = "planned".into();
        report["dry_run"] = true.into();
        report["planned_steps"] = serde_json::json!([
            "按 Public → Admin → Worker 停止三角色",
            "创建停机备份并用新旧两个 auction-cli 各校验一次",
            "把当前程序保留到部署状态目录的回滚记录",
            "逐个原子替换为制品程序（四个程序整体不是原子的；中途失败时从回滚记录换回已替换的程序，不迁移、不启动）",
            "用迁移 owner 执行迁移",
            "按 Worker → Admin → Public 启动并等待 readiness",
            "复核运行程序的摘要与制品一致",
            "追加部署回执",
        ]);
        return print_json(&report);
    }

    // 停机前最后一道门：锁、回执与回滚记录必须仍属于持锁时固定的状态目录。
    state.verify_unchanged()?;
    let shutdown_gate = serde_json::to_value(&gate)?;
    let plan = DeployPlan {
        config_dir,
        units: &units,
        state: state.directory(),
        systemctl: &systemctl,
        deployment: &deployment,
        owner: &owner,
        artifact,
        install_dir: &install_dir,
        backup_dir: &backup_dir,
        database: &database,
        current_version,
        expected_version,
        migration_drill_id: drill.as_ref().map(|drill| drill.drill_id.as_str()),
        startup_timeout: Duration::from_secs(startup_timeout_seconds),
        started_at,
    };
    match deploy_locked(&plan, &mut context, shutdown_gate).await {
        Ok(value) => {
            report["outcome"] = "started".into();
            report["deployment"] = value;
            print_json(&report)
        }
        Err(failure) => Err(settle(
            state.directory(),
            &systemctl,
            Action::Deploy,
            started_at,
            &context,
            failure,
        )
        .await),
    }
}
/// 从最新成功且三角色摘要匹配的部署回执取得旧角色身份与包名；未知包名保留为 None。
fn installed_roles_identity(
    state: &PrivateDirectory,
    install_dir: &Path,
) -> anyhow::Result<(serde_json::Value, Option<String>)> {
    let binaries = install_dir_artifacts(install_dir)
        .into_iter()
        .filter_map(|artifact| {
            let role = service::ManagedRole::START_ORDER
                .into_iter()
                .find(|role| role.name() == artifact.role)?;
            Some(RetainedBinary {
                name: role_binary(role).to_owned(),
                sha256: artifact.sha256?,
                bytes: artifact.bytes?,
            })
        })
        .collect::<Vec<_>>();
    let identity = installed_identity(state, &binaries);
    let package = identity.roles.package.clone();
    Ok((serde_json::to_value(identity.roles)?, package))
}

fn drill_package_inputs<'a>(
    package_sha256: Option<&'a str>,
    previous_package: Option<&'a str>,
) -> anyhow::Result<(&'a str, &'a str)> {
    let package_sha256 =
        package_sha256.context("含迁移部署必须提供已校验 Release tarball 的 --package-sha256")?;
    if !is_lower_hex(package_sha256, 64) {
        anyhow::bail!("--package-sha256 必须是 64 位小写十六进制 SHA-256");
    }
    let previous_package = previous_package
        .filter(|package| !package.is_empty() && !package.chars().any(char::is_control))
        .context("无法从摘要匹配的成功部署回执确定当前角色制品包；拒绝迁移部署")?;
    Ok((previous_package, package_sha256))
}

#[cfg(test)]
mod tests;
