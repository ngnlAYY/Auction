//! `auction-cli deploy <制品目录>`：分体部署的一条命令编排。
//!
//! 顺序：制品静态校验（文件白名单、权限、摘要、Web payload 标记）→ 严格配置与 unit 绑定预检
//! → 时段门禁 → 迁移版本与演练回执 → 在途发送门禁 → 停机 → 停机备份并由新旧
//! CLI 各校验一次 → 保留旧程序 → 原子安装 → 迁移与顺序启动 → 运行程序摘要复核 → 部署回执。
//! 任一门禁失败都在停机前拒绝；安装后启动失败只停止本次启动的 unit，并把回退指引写入回执。

use std::{path::Path, time::Duration};

use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

use crate::app::{print_json, progress};
use crate::cli::DeployArgs;
use crate::config_dir::{ConfigDir, Deployment};
use crate::receipt::{self, Action, DeployRecord, DeploymentReceipt, Outcome, ReceiptContext};
use crate::service::{self, ManagedRole, Systemctl, UnitState};

pub(crate) mod artifact;
mod drill;
mod gates;
mod install;
mod rollback;

pub(crate) use rollback::run as run_rollback;

pub(crate) async fn run(config_dir: &ConfigDir, args: DeployArgs) -> anyhow::Result<()> {
    let DeployArgs {
        artifact_dir,
        units,
        owner,
        shutdown,
        receipt: receipt_args,
        install_dir: explicit_install_dir,
        backup_dir,
        migration_drill_receipt,
        drill_max_age_days,
        send_gate_timeout_seconds,
        startup_timeout_seconds,
        dry_run,
    } = args;
    let artifact = artifact::inspect(&artifact_dir).context("部署制品校验失败")?;
    require_artifact_cli(&artifact)?;
    let mut context = ReceiptContext::resolve(None, receipt_args.actor.as_deref())?;
    let started_at = Utc::now();
    let owner = crate::database::load_owner_secrets(&owner.owner_secrets)?;
    let systemctl = Systemctl::new(&units)?;
    let _lock = crate::service::deployment_lock(&units)?;
    let deployment = service::prepare(config_dir, &systemctl).await?;
    let states = unit_states(&systemctl).await?;
    let running = running_roles(&states);
    let install_dir = gates::resolve_install_dir(explicit_install_dir.as_deref(), &running)?;
    install::require_install_dir(&install_dir)?;
    let service_uid = shared_service_uid(&states)?;

    let gate =
        service::shutdown_gate(config_dir, Some(&deployment.worker), "deploy", &shutdown).await?;
    let database = gates::database_name(&deployment.worker)?;
    let expected_version = auction_storage::postgres_expected_migration_version();
    let current_version = gates::database_version(&deployment.worker).await?;
    if expected_version < current_version {
        anyhow::bail!(
            "制品内嵌 schema 版本 {expected_version} 低于数据库 {current_version}；\
             回退请使用 auction-cli rollback，含破坏性迁移只能按 ops/deploy/RECOVERY.md 恢复"
        );
    }
    // 旧程序此刻正在服务版本 current_version 的数据库，因此它的有效基线就是当前版本；
    // 不额外调用旧 CLI（新配置可能含旧程序不认识的字段），也不把未证实的版本写进回滚记录。
    let previous_version = current_version;
    let drill = if expected_version > current_version {
        let path = migration_drill_receipt.as_deref().with_context(|| {
            format!(
                "本次制品包含迁移（数据库 {current_version} → {expected_version}），\
                 必须提供 --migration-drill-receipt（W10.6）"
            )
        })?;
        Some(
            drill::inspect(
                path,
                &drill::Requirements {
                    artifact: &artifact,
                    current_version,
                    expected_version,
                    protection_window_minutes: shutdown.protection_window_minutes,
                    max_age_days: drill_max_age_days,
                    now: Utc::now(),
                },
            )
            .await
            .context("迁移演练回执核对失败")?,
        )
    } else {
        None
    };
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
        "previous_version": previous_version,
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
        "artifact": artifact_projection(&artifact),
        "install_directory": install_dir,
        "backup_directory": backup_dir,
        "gates": {
            "shutdown": gate,
            "versions": versions,
            "migration_drill": drill_report,
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
            "以原子替换安装制品程序",
            "用迁移 owner 执行迁移",
            "按 Worker → Admin → Public 启动并等待 readiness",
            "复核运行程序的摘要与制品一致",
            "追加部署回执",
        ]);
        return print_json(&report);
    }

    require_backup_dir(&backup_dir)?;
    let outcome = deploy_locked(
        config_dir,
        &units,
        &systemctl,
        &deployment,
        &owner,
        &artifact,
        &install_dir,
        &backup_dir,
        &database,
        previous_version,
        expected_version,
        drill.as_ref().map(|drill| drill.drill_id.as_str()),
        Duration::from_secs(startup_timeout_seconds),
        service_uid,
        &mut context,
        &gate,
        started_at,
    )
    .await;
    match outcome {
        Ok(value) => {
            report["outcome"] = "started".into();
            report["deployment"] = value;
            print_json(&report)
        }
        Err(error) => {
            record_failure(&units, &systemctl, Action::Deploy, started_at, &context, &error)
                .await?;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn deploy_locked(
    config_dir: &ConfigDir,
    units: &crate::cli::UnitArgs,
    systemctl: &Systemctl,
    deployment: &Deployment,
    owner: &auction_config::PostgresOwnerSecrets,
    artifact: &artifact::Artifact,
    install_dir: &Path,
    backup_dir: &Path,
    database: &str,
    previous_version: i64,
    expected_version: i64,
    migration_drill_id: Option<&str>,
    startup_timeout: Duration,
    service_uid: u32,
    context: &mut ReceiptContext,
    gate: &service::ShutdownGateReport<'_>,
    started_at: chrono::DateTime<Utc>,
) -> anyhow::Result<serde_json::Value> {
    progress("停止三角色（Public → Admin → Worker）");
    service::stop_all(systemctl).await?;

    let archive =
        backup_dir.join(format!("auction-deploy-{}.dump", Utc::now().format("%Y%m%dT%H%M%SZ")));
    progress(format_args!("创建停机备份：{}", archive.display()));
    let manifest = gates::create_backup(&deployment.worker, database, &archive)
        .await
        .context("创建停机备份失败")?;
    let manifest_path = archive.with_file_name(format!("{}.manifest", manifest.archive_file_name));
    let new_cli = artifact.binary("auction-cli").context("制品缺少 auction-cli")?.path.clone();
    let old_cli = install_dir.join("auction-cli");
    progress("用旧 CLI 校验备份");
    gates::verify_backup_with_cli(&old_cli, config_dir.path(), &manifest_path, database)
        .await
        .context("旧 CLI 校验备份失败")?;
    progress("用新 CLI 校验备份");
    gates::verify_backup_with_cli(&new_cli, config_dir.path(), &manifest_path, database)
        .await
        .context("新 CLI 校验备份失败")?;
    context.set_backup_id(Some(&manifest.archive_file_name));

    // 回滚记录与其部署回执必须共享同一标识（`rollback` 按 `deploy_receipt_id` 读回执），
    // 因此在保留旧程序前先分配，成功后用同一标识发布回执。
    let deploy_receipt_id = Uuid::new_v4();
    progress("保留当前程序");
    let retained = install::retain(
        &units.state_dir,
        deploy_receipt_id,
        install_dir,
        previous_version,
        expected_version,
        previous_source_commit(units).as_deref(),
    )
    .context("保留当前程序失败")?;
    progress(format_args!("安装制品程序到 {}", install_dir.display()));
    install::install_artifact(artifact, install_dir, service_uid)?;

    // 启动前先构造部署证据，保证启动失败时仍能用同一回执 ID 发布可回滚记录。
    let deploy = DeployRecord {
        artifact_directory: artifact.root.display().to_string(),
        package: artifact.package.clone(),
        source_commit: artifact.source_commit.clone(),
        source_fingerprint: artifact.source_fingerprint.clone(),
        cli_sha256: artifact.cli_sha256().to_owned(),
        install_directory: install_dir.display().to_string(),
        public_payload_sha256: artifact.public_payload_sha256.clone(),
        admin_payload_sha256: artifact.admin_payload_sha256.clone(),
        migration_drill_id: migration_drill_id.map(str::to_owned),
        backup_archive: Some(archive.display().to_string()),
        backup_manifest: Some(manifest_path.display().to_string()),
        rollback_record_id: Some(retained.record_id),
    };
    progress("迁移并顺序启动三角色");
    let launch = match service::launch(
        systemctl,
        deployment,
        Some(owner),
        startup_timeout,
        Some(serde_json::to_value(gate)?),
    )
    .await
    {
        Ok(launch) => launch,
        Err(error) => {
            let failure = error.context(format!(
                "新程序已安装但启动失败；已停止本次启动的 unit，回退指引见 \
                 auction-cli rollback --receipt-id {deploy_receipt_id}"
            ));
            // 回滚记录已指向 deploy_receipt_id；必须先发布失败部署回执，rollback 才能读到
            // 已安装制品与保留记录。失败回执的 artifacts 来自已安装制品摘要，不伪造运行进程。
            let artifacts = installed_artifacts(artifact)?;
            receipt::record_deploy(
                &units.state_dir,
                deploy_receipt_id,
                Action::Deploy,
                Outcome::Failed,
                started_at,
                context,
                None,
                artifacts,
                deploy,
                Some(receipt::failure_text(&failure)),
            )
            .context("写入失败部署回执失败")?;
            return Err(failure);
        }
    };
    let artifacts = verify_running_artifacts(systemctl, artifact).await?;
    let published = receipt::record_deploy(
        &units.state_dir,
        deploy_receipt_id,
        Action::Deploy,
        Outcome::Succeeded,
        started_at,
        context,
        launch.migration.as_ref().map(|migration| migration.record.clone()),
        artifacts,
        deploy,
        None,
    )
    .context("写入部署回执失败")?;
    Ok(serde_json::json!({
        "backup": {
            "archive": archive,
            "manifest": manifest_path,
            "archive_sha256": manifest.archive_sha256,
            "schema_version": manifest.schema_version,
            "verified_by": ["installed_cli", "artifact_cli"],
        },
        "rollback_record": install::record_projection(&retained),
        "migration": launch.migration.as_ref().map(|migration| &migration.report),
        "roles": launch.roles,
        "deployment_receipt": receipt_summary(units, &published),
    }))
}

/// 已安装制品的静态回执；启动失败时没有可核对的运行进程，只能用制品摘要证明回滚目标。
fn installed_artifacts(artifact: &artifact::Artifact) -> anyhow::Result<Vec<receipt::Artifact>> {
    let mut artifacts = Vec::with_capacity(ManagedRole::START_ORDER.len());
    for role in ManagedRole::START_ORDER {
        let binary = artifact
            .binary(role_binary(role))
            .with_context(|| format!("制品缺少 {} 程序", role.name()))?;
        artifacts.push(receipt::Artifact {
            role: role.name().to_owned(),
            path: Some(binary.path.display().to_string()),
            sha256: Some(binary.sha256.clone()),
            bytes: Some(binary.bytes),
        });
    }
    Ok(artifacts)
}

/// 运行中的程序摘要必须与制品完全一致；这也是内嵌静态资源已生效的证据。
async fn verify_running_artifacts(
    systemctl: &Systemctl,
    artifact: &artifact::Artifact,
) -> anyhow::Result<Vec<receipt::Artifact>> {
    let mut artifacts = Vec::with_capacity(ManagedRole::START_ORDER.len());
    for role in ManagedRole::START_ORDER {
        let state = systemctl.show(role).await?;
        if !state.running() {
            anyhow::bail!("{} 未运行，无法核对程序摘要", role.name());
        }
        let record = receipt::artifact_from_process(role.name(), state.main_pid);
        let expected = artifact
            .binary(role_binary(role))
            .with_context(|| format!("制品缺少 {} 程序", role.name()))?;
        if record.sha256.as_deref() != Some(expected.sha256.as_str()) {
            anyhow::bail!(
                "{} 运行的程序摘要与制品不一致；请核对安装目录与 unit ExecStart",
                role.name()
            );
        }
        artifacts.push(record);
    }
    Ok(artifacts)
}

pub(crate) const fn role_binary(role: ManagedRole) -> &'static str {
    match role {
        ManagedRole::Worker => "auction-worker",
        ManagedRole::Admin => "auction-admin-api",
        ManagedRole::Public => "auction-public-api",
    }
}

pub(crate) async fn unit_states(systemctl: &Systemctl) -> anyhow::Result<[UnitState; 3]> {
    let mut states = Vec::with_capacity(3);
    for role in ManagedRole::START_ORDER {
        states.push(systemctl.show(role).await?);
    }
    states.try_into().ok().context("三角色状态数量不一致")
}

pub(crate) fn running_roles(states: &[UnitState; 3]) -> Vec<(&'static str, u32)> {
    ManagedRole::START_ORDER
        .iter()
        .zip(states)
        .filter(|(_, state)| state.running())
        .map(|(role, state)| (role.name(), state.main_pid))
        .collect()
}

/// 三个 unit 必须使用同一个固定服务账号；DynamicUser 或账号不一致都不能原子安装。
pub(crate) fn shared_service_uid(states: &[UnitState; 3]) -> anyhow::Result<u32> {
    let mut uid = None;
    for (role, state) in ManagedRole::START_ORDER.iter().zip(states) {
        if state.dynamic_user {
            anyhow::bail!("{} unit 使用 DynamicUser，无法为程序设置稳定属主", role.name());
        }
        let current = service::service_uid(&state.user)?;
        match uid {
            Some(expected) if expected != current => {
                anyhow::bail!("三角色 unit 必须使用同一个服务账号才能原子安装程序");
            }
            Some(_) => {}
            None => uid = Some(current),
        }
    }
    uid.context("无法确定服务账号")
}

fn require_artifact_cli(artifact: &artifact::Artifact) -> anyhow::Result<()> {
    let current = std::env::current_exe().context("读取当前 auction-cli 路径失败")?;
    let (sha256, _) = artifact::sha256_path(&current).context("计算当前 auction-cli 摘要失败")?;
    if sha256 != artifact.cli_sha256() {
        anyhow::bail!(
            "deploy 必须由制品内的 auction-cli 执行（{}）；当前程序摘要与制品不一致",
            artifact.binary("auction-cli").map_or_else(
                || "bin/auction-cli".to_owned(),
                |binary| binary.path.display().to_string()
            )
        );
    }
    Ok(())
}

/// 备份目录必须已存在、非符号链接，且不允许 group/other 写入。
fn require_backup_dir(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("--backup-dir 必须是绝对路径");
    }
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("读取备份目录失败：{}", path.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("备份目录必须是非符号链接目录：{}", path.display());
    }
    use std::os::unix::fs::MetadataExt;
    if metadata.mode() & 0o022 != 0 {
        anyhow::bail!("备份目录不得允许 group/other 写入：{}", path.display());
    }
    Ok(())
}

pub(crate) async fn record_failure(
    units: &crate::cli::UnitArgs,
    systemctl: &Systemctl,
    action: Action,
    started_at: chrono::DateTime<Utc>,
    context: &ReceiptContext,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    let failure = receipt::failure_text(error);
    let mut artifacts = Vec::new();
    for role in ManagedRole::START_ORDER {
        let Ok(state) = systemctl.show(role).await else { continue };
        if state.running() {
            artifacts.push(receipt::artifact_from_process(role.name(), state.main_pid));
        }
    }
    if let Err(record_error) = receipt::record(
        &units.state_dir,
        action,
        Outcome::Failed,
        started_at,
        context,
        None,
        artifacts,
        Some(failure),
    ) {
        return Err(anyhow::anyhow!("{error:#}；部署回执也未写入：{record_error:#}"));
    }
    Ok(())
}

pub(crate) fn receipt_summary(
    units: &crate::cli::UnitArgs,
    receipt: &DeploymentReceipt,
) -> serde_json::Value {
    serde_json::json!({
        "receipt_id": receipt.receipt_id,
        "path": receipt::receipts_dir(&units.state_dir)
            .join(format!("{}.json", receipt.receipt_id)),
        "outcome": receipt.outcome,
    })
}

pub(crate) fn artifact_projection(artifact: &artifact::Artifact) -> serde_json::Value {
    serde_json::json!({
        "directory": artifact.root,
        "package": artifact.package,
        "source_commit": artifact.source_commit,
        "source_fingerprint": artifact.source_fingerprint,
        "binaries": artifact.binaries.iter().map(|binary| serde_json::json!({
            "name": binary.name,
            "sha256": binary.sha256,
            "bytes": binary.bytes,
        })).collect::<Vec<_>>(),
        "public_payload_sha256": artifact.public_payload_sha256,
        "admin_payload_sha256": artifact.admin_payload_sha256,
    })
}

/// 最近一次成功部署的源码提交；用于回滚记录的可追溯性，缺失时不伪造。
fn previous_source_commit(units: &crate::cli::UnitArgs) -> Option<String> {
    let receipts = receipt::list(&units.state_dir, 20).ok()?;
    receipts
        .into_iter()
        .find(|receipt| receipt.outcome == Outcome::Succeeded)
        .and_then(|receipt| receipt.deploy)
        .map(|deploy| deploy.source_commit)
}
