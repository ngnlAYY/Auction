//! 持锁执行的部署主体：停机、替换、迁移与启动。
//!
//! 停机后的失败分两段收口：保留旧程序之前返回未记录的失败，由入口追加通用失败回执；保留成功后
//! 的安装、启动、运行核对与成功回执写入失败，都以回滚记录的标识发布一条失败回执（回执本身
//! 写不进去时错误注明“部署回执也未写入”）。

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use auction_config::PostgresOwnerSecrets;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::artifact::REQUIRED_BINARIES;
use super::install::RetainRequest;
use super::receipts::{FailedReceipt, Failure, installed_identity};
use super::verification::{install_dir_artifacts, installed_artifacts, verify_running_artifacts};
use super::{artifact, gates, install};
use crate::app::progress;
use crate::cli::UnitArgs;
use crate::config_dir::{ConfigDir, Deployment};
use crate::receipt::{Action, CliIdentity, DeployRecord, Outcome, ReceiptContext, RoleIdentity};
use crate::service::Systemctl;
use crate::support::files::PrivateDirectory;
use crate::{receipt, service};

/// 停机前已核对完毕的部署输入；停机后的步骤只读取它，不再做新的门禁判定。
pub(super) struct DeployPlan<'a> {
    pub(super) config_dir: &'a ConfigDir,
    pub(super) units: &'a UnitArgs,
    /// 持锁时固定的部署状态目录；回滚记录与回执都写在它的描述符下。
    pub(super) state: &'a PrivateDirectory,
    pub(super) systemctl: &'a Systemctl,
    pub(super) deployment: &'a Deployment,
    pub(super) owner: &'a PostgresOwnerSecrets,
    pub(super) artifact: &'a artifact::Artifact,
    pub(super) install_dir: &'a Path,
    pub(super) backup_dir: &'a Path,
    pub(super) database: &'a str,
    /// 停机前门禁观测到的数据库迁移版本（迁移之前）。
    pub(super) current_version: i64,
    /// 制品程序内嵌的期望迁移版本；四个程序同属一个制品。
    pub(super) expected_version: i64,
    pub(super) migration_drill_id: Option<&'a str>,
    pub(super) startup_timeout: Duration,
    pub(super) started_at: DateTime<Utc>,
}

pub(super) async fn deploy_locked(
    plan: &DeployPlan<'_>,
    context: &mut ReceiptContext,
    shutdown_gate: serde_json::Value,
) -> Result<serde_json::Value, Failure> {
    let DeployPlan { units, state, systemctl, artifact, install_dir, database, .. } = *plan;
    progress("停止三角色（Public → Admin → Worker）");
    service::stop_all(systemctl).await.map_err(Failure::unrecorded)?;

    let archive = plan
        .backup_dir
        .join(format!("auction-deploy-{}.dump", Utc::now().format("%Y%m%dT%H%M%SZ")));
    progress(format_args!("创建停机备份：{}", archive.display()));
    let manifest = crate::backup::create_backup(&plan.deployment.worker, database, &archive)
        .await
        .context("创建停机备份失败")
        .map_err(|error| stopped_before_install(error, install_dir))?;
    let manifest_path = archive.with_file_name(format!("{}.manifest", manifest.archive_file_name));
    let new_cli = artifact
        .binary("auction-cli")
        .context("制品缺少 auction-cli")
        .map_err(|error| stopped_before_install(error, install_dir))?
        .path
        .clone();
    let old_cli = install_dir.join("auction-cli");
    progress("用旧 CLI 校验备份");
    gates::verify_backup_with_cli(&old_cli, plan.config_dir.path(), &manifest_path, database)
        .await
        .context("旧 CLI 校验备份失败")
        .map_err(|error| stopped_before_install(error, install_dir))?;
    progress("用新 CLI 校验备份");
    gates::verify_backup_with_cli(&new_cli, plan.config_dir.path(), &manifest_path, database)
        .await
        .context("新 CLI 校验备份失败")
        .map_err(|error| stopped_before_install(error, install_dir))?;
    context.set_backup_id(Some(&manifest.archive_file_name));
    let context: &ReceiptContext = context;

    // 回滚记录与其部署回执必须共享同一标识（`rollback` 按 `deploy_receipt_id` 读回执），
    // 因此在保留旧程序前先分配，成功或失败都用同一标识发布回执。
    let deploy_receipt_id = Uuid::new_v4();
    progress("保留当前程序");
    // 旧程序的身份按保留副本的实际摘要从回执绑定；数据库版本只作为观测值另行记录。
    let request = RetainRequest {
        deploy_receipt_id,
        install_dir,
        programs: &REQUIRED_BINARIES,
        observed_database_version: plan.current_version,
        installed_role_schema_version: Some(plan.expected_version),
    };
    let (retained, record_directory) =
        install::retain(state, &request, |binaries| Ok(installed_identity(state, binaries)))
            .context("保留当前程序失败")
            .map_err(|error| stopped_before_install(error, install_dir))?;

    // 回滚记录已指向 deploy_receipt_id：此后的任何失败都必须用同一标识发布带回滚证据的失败
    // 回执，rollback 才能读回已安装制品与保留记录；入口不再追加第二条回执。
    let deploy = deploy_record(plan, &archive, &manifest_path, retained.record_id);
    let failed = FailedReceipt {
        state,
        receipt_id: deploy_receipt_id,
        action: Action::Deploy,
        started_at: plan.started_at,
        context,
        deploy: &deploy,
    };
    progress(format_args!("安装制品程序到 {}", install_dir.display()));
    if let Err(failure) =
        install::install_artifact(artifact, install_dir, &retained, &record_directory)
    {
        let failure = install_failed(failure, install_dir, record_directory.path());
        // 失败回执的 artifacts 记录安装目录实际文件的摘要（已换回时是旧程序），不冒充制品摘要；
        // deploy 段仍是尝试安装的制品，失败回执从不作为组件身份来源。已换回时本次回滚记录保留的
        // 就是当前程序，rollback 会因目标与当前程序相同而在停机前拒绝。
        return Err(failed.publish(failure, None, Ok(install_dir_artifacts(install_dir))));
    }

    progress("迁移并顺序启动三角色");
    let launch = match service::launch(
        systemctl,
        plan.deployment,
        Some(plan.owner),
        plan.startup_timeout,
        Some(shutdown_gate),
    )
    .await
    {
        Ok(launch) => launch,
        Err(error) => {
            // 迁移可能尚未完成：rollback 保留新 CLI，会在停机前拒绝比数据库新的 CLI，因此同时
            // 给出迁移前失败的处置入口。
            let failure = error.context(format!(
                "新程序已安装但启动失败；已停止本次启动的 unit。迁移已完成时用安装目录的 \
                 auction-cli rollback --receipt-id {deploy_receipt_id} 回退；迁移未完成时 rollback \
                 会拒绝，修复原因后用安装目录的 CLI 执行 service start 完成升级，或按 \
                 ops/deploy/RECOVERY.md 的“安装中途失败的手工换回”用本次回滚记录整体换回"
            ));
            // 没有可核对的运行进程：失败回执的 artifacts 来自已安装制品摘要，不伪造运行进程。
            return Err(failed.publish(failure, None, installed_artifacts(artifact)));
        }
    };
    let migration = launch.migration.as_ref().map(|migration| migration.record.clone());
    let artifacts = match verify_running_artifacts(systemctl, artifact).await {
        Ok(artifacts) => artifacts,
        Err(error) => {
            let failure = error.context(format!(
                "新程序已启动但运行程序核对失败；回退指引见 \
                 auction-cli rollback --receipt-id {deploy_receipt_id}"
            ));
            return Err(failed.publish(failure, migration, installed_artifacts(artifact)));
        }
    };
    let published = match receipt::record_deploy(
        state,
        deploy_receipt_id,
        Action::Deploy,
        Outcome::Succeeded,
        plan.started_at,
        context,
        migration.clone(),
        artifacts.clone(),
        deploy.clone(),
        None,
    ) {
        Ok(published) => published,
        // 成功回执未写入时以同一标识补写失败回执；若成功回执其实已发布，无覆盖发布会拒绝
        // 补写，两段原因一并返回。
        Err(error) => {
            return Err(failed.publish(
                error.context("写入部署回执失败"),
                migration,
                Ok(artifacts),
            ));
        }
    };
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
        "deployment_receipt": service::receipt_summary(units, &published),
    }))
}

/// deploy 回执的 v2 deploy 段：四个程序同属本次制品，角色与 CLI 的身份与内嵌 schema 都来自它；
/// 数据库版本是迁移之前的观测值。
fn deploy_record(
    plan: &DeployPlan<'_>,
    archive: &Path,
    manifest: &Path,
    rollback_record_id: Uuid,
) -> DeployRecord {
    let artifact = plan.artifact;
    DeployRecord {
        install_directory: plan.install_dir.display().to_string(),
        roles: RoleIdentity {
            artifact_directory: Some(artifact.root.display().to_string()),
            package: Some(artifact.package.clone()),
            source_commit: Some(artifact.source_commit.clone()),
            source_fingerprint: Some(artifact.source_fingerprint.clone()),
            public_payload_sha256: Some(artifact.public_payload_sha256.clone()),
            admin_payload_sha256: Some(artifact.admin_payload_sha256.clone()),
            schema_version: Some(plan.expected_version),
        },
        cli: CliIdentity {
            sha256: artifact.cli_sha256().to_owned(),
            package: Some(artifact.package.clone()),
            source_commit: Some(artifact.source_commit.clone()),
            schema_version: Some(plan.expected_version),
        },
        database_version: plan.current_version,
        migration_drill_id: plan.migration_drill_id.map(str::to_owned),
        backup_archive: Some(archive.display().to_string()),
        backup_manifest: Some(manifest.display().to_string()),
        rollback_record_id: Some(rollback_record_id),
    }
}

/// 停机后、安装前的失败：安装目录与数据库都未改变，只差重新启动旧程序。制品 CLI 会先把 schema
/// 迁到新版本，而安装目录仍是旧程序，所以错误里给出安装目录中旧 CLI 的恢复命令。
fn stopped_before_install(error: anyhow::Error, install_dir: &Path) -> Failure {
    Failure::unrecorded(error.context(format!(
        "部署在停机后、安装前中止：三角色已停止，{} 中仍是旧程序，迁移尚未执行；{}",
        install_dir.display(),
        restart_old_version(install_dir)
    )))
}

/// 安装中途失败：迁移尚未执行，已替换的程序已尝试从回滚记录换回。换回并复核成功时与安装前
/// 失败的处置相同；否则安装目录可能新旧混合，只给出人工换回的入口，不给启动命令。
fn install_failed(
    failure: install::InstallArtifactFailure,
    install_dir: &Path,
    record_dir: &Path,
) -> anyhow::Error {
    if failure.restored {
        return failure.error.context(format!(
            "安装制品程序失败：三角色已停止，迁移尚未执行，{} 中的四个程序已复核为部署前的旧程序\
             （换回明细见下）；{}",
            install_dir.display(),
            restart_old_version(install_dir)
        ));
    }
    failure.error.context(format!(
        "安装制品程序失败且自动换回未完成：三角色已停止，迁移尚未执行；{} 中的程序可能新旧混合，\
         旧程序完整保留在 {}（摘要见 {}）。按 ops/deploy/RECOVERY.md 的“安装中途失败的手工换回”\
         核对摘要后再启动；不要用制品 CLI 执行 service start（它会先执行新迁移）",
        install_dir.display(),
        record_dir.join("bin").display(),
        record_dir.join(install::RECORD_FILE).display()
    ))
}

/// 安装目录是旧程序、数据库未迁移时的恢复命令：只能用安装目录中的旧 CLI 启动。
fn restart_old_version(install_dir: &Path) -> String {
    format!(
        "恢复旧版请用 {} service start（沿用本次的 --config-dir、--owner-secrets、--state-dir 与 \
         unit 参数），不要用制品 CLI（它会先把 schema 迁到新版本，而安装目录仍是旧程序）",
        install_dir.join("auction-cli").display()
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{install_failed, stopped_before_install};
    use crate::deploy::install::InstallArtifactFailure;

    /// 停机后、安装前的失败交给入口写通用回执，错误给出安装目录中旧 CLI 的恢复命令（含
    /// --state-dir，漏传会让部署锁与回执落到默认目录）并警告不要用制品 CLI；原始原因保留在
    /// 错误链末尾。
    #[test]
    fn stopped_before_install_names_the_installed_cli() {
        let failure = stopped_before_install(
            anyhow::anyhow!("pg_dump 失败").context("创建停机备份失败"),
            Path::new("/opt/auction/bin"),
        );
        assert!(!failure.recorded);
        let text = format!("{:#}", failure.error);
        assert!(text.contains("/opt/auction/bin/auction-cli service start"), "{text}");
        assert!(text.contains("--state-dir"), "{text}");
        assert!(text.contains("不要用制品 CLI") && text.contains("迁移尚未执行"), "{text}");
        assert!(text.contains("安装目录仍是旧程序"), "{text}");
        assert!(text.ends_with("创建停机备份失败: pg_dump 失败"), "{text}");
    }

    /// 安装中途失败的指引取决于自动换回：已换回并复核时给出安装目录 CLI 的启动命令；未完成
    /// 时只指向人工换回与回滚记录，不给任何启动命令。
    #[test]
    fn install_failure_guidance_follows_the_swap_back_result() {
        let install_dir = Path::new("/opt/auction/bin");
        let record_dir = Path::new("/var/lib/auction/deployment/rollback/r1");
        let restored = install_failed(
            InstallArtifactFailure {
                error: anyhow::anyhow!("原子替换程序失败"), restored: true
            },
            install_dir,
            record_dir,
        );
        let text = format!("{restored:#}");
        assert!(text.contains("已复核为部署前的旧程序") && text.contains("迁移尚未执行"), "{text}");
        assert!(text.contains("/opt/auction/bin/auction-cli service start"), "{text}");
        assert!(text.contains("不要用制品 CLI"), "{text}");

        let mixed = install_failed(
            InstallArtifactFailure {
                error: anyhow::anyhow!("原子替换程序失败"), restored: false
            },
            install_dir,
            record_dir,
        );
        let text = format!("{mixed:#}");
        assert!(text.contains("自动换回未完成") && text.contains("可能新旧混合"), "{text}");
        assert!(text.contains("安装中途失败的手工换回"), "{text}");
        assert!(text.contains("/var/lib/auction/deployment/rollback/r1/bin"), "{text}");
        assert!(!text.contains("/opt/auction/bin/auction-cli service start"), "{text}");
        assert!(text.ends_with("原子替换程序失败"), "{text}");
    }
}
