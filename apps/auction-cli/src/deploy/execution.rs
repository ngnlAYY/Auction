//! 持锁执行的部署主体：停机、替换、迁移与启动。
//!
//! 停机后的失败分两段收口：保留旧程序之前返回未记录的失败，由入口追加通用失败回执；保留成功后
//! 的安装、启动、运行核对与成功回执写入失败，都以回滚记录的标识发布恰好一条失败回执。

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

use super::receipts::{FailedReceipt, Failure, previous_source_commit};
use super::verification::{install_dir_artifacts, installed_artifacts, verify_running_artifacts};
use super::{artifact, gates, install};
use crate::app::progress;
use crate::config_dir::{ConfigDir, Deployment};
use crate::receipt::{Action, DeployRecord, Outcome, ReceiptContext};
use crate::service::Systemctl;
use crate::{receipt, service};

#[allow(clippy::too_many_arguments)]
pub(super) async fn deploy_locked(
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
    shutdown_gate: serde_json::Value,
    started_at: chrono::DateTime<Utc>,
) -> Result<serde_json::Value, Failure> {
    progress("停止三角色（Public → Admin → Worker）");
    service::stop_all(systemctl).await.map_err(Failure::unrecorded)?;

    let archive =
        backup_dir.join(format!("auction-deploy-{}.dump", Utc::now().format("%Y%m%dT%H%M%SZ")));
    progress(format_args!("创建停机备份：{}", archive.display()));
    let manifest = gates::create_backup(&deployment.worker, database, &archive)
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
    gates::verify_backup_with_cli(&old_cli, config_dir.path(), &manifest_path, database)
        .await
        .context("旧 CLI 校验备份失败")
        .map_err(|error| stopped_before_install(error, install_dir))?;
    progress("用新 CLI 校验备份");
    gates::verify_backup_with_cli(&new_cli, config_dir.path(), &manifest_path, database)
        .await
        .context("新 CLI 校验备份失败")
        .map_err(|error| stopped_before_install(error, install_dir))?;
    context.set_backup_id(Some(&manifest.archive_file_name));
    let context: &ReceiptContext = context;

    // 回滚记录与其部署回执必须共享同一标识（`rollback` 按 `deploy_receipt_id` 读回执），
    // 因此在保留旧程序前先分配，成功或失败都用同一标识发布回执。
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
    .context("保留当前程序失败")
    .map_err(|error| stopped_before_install(error, install_dir))?;

    // 回滚记录已指向 deploy_receipt_id：此后的任何失败都必须用同一标识发布带回滚证据的失败
    // 回执，rollback 才能读回已安装制品与保留记录；入口不再追加第二条回执。
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
    let failed = FailedReceipt {
        state_dir: &units.state_dir,
        receipt_id: deploy_receipt_id,
        action: Action::Deploy,
        started_at,
        context,
        deploy: &deploy,
    };
    progress(format_args!("安装制品程序到 {}", install_dir.display()));
    if let Err(error) = install::install_artifact(artifact, install_dir, service_uid) {
        let record_dir = install::record_dir(&units.state_dir, retained.record_id);
        let failure = error.context(format!(
            "安装制品程序失败：三角色已停止，迁移尚未执行；{} 中的程序可能已部分替换为新版本，\
             旧程序完整保留在 {}（摘要见 {}）。不要用制品 CLI 执行 service start（它会先执行\
             新迁移）",
            install_dir.display(),
            record_dir.join("bin").display(),
            record_dir.join(install::RECORD_FILE).display()
        ));
        // 安装目录可能新旧混合：失败回执记录实际文件摘要，不冒充制品摘要；
        // 之后 rollback 会在安装目录核对处失败关闭。
        return Err(failed.publish(failure, None, Ok(install_dir_artifacts(install_dir))));
    }

    progress("迁移并顺序启动三角色");
    let launch = match service::launch(
        systemctl,
        deployment,
        Some(owner),
        startup_timeout,
        Some(shutdown_gate),
    )
    .await
    {
        Ok(launch) => launch,
        Err(error) => {
            let failure = error.context(format!(
                "新程序已安装但启动失败；已停止本次启动的 unit，回退指引见 \
                 auction-cli rollback --receipt-id {deploy_receipt_id}"
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
        &units.state_dir,
        deploy_receipt_id,
        Action::Deploy,
        Outcome::Succeeded,
        started_at,
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

/// 停机后、安装前的失败：安装目录与数据库都未改变，只差重新启动旧程序。制品 CLI 会先执行
/// 新迁移，而此时没有可用的停机备份，所以错误里给出安装目录中旧 CLI 的恢复命令。
fn stopped_before_install(error: anyhow::Error, install_dir: &Path) -> Failure {
    Failure::unrecorded(error.context(format!(
        "部署在停机后、安装前中止：三角色已停止，{} 中仍是旧程序，迁移尚未执行；恢复旧版请用 \
         {} service start（沿用本次的 --config-dir、--owner-secrets 与 unit 参数），\
         不要用制品 CLI（它会先执行新迁移）",
        install_dir.display(),
        install_dir.join("auction-cli").display()
    )))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::stopped_before_install;

    /// 停机后、安装前的失败交给入口写通用回执，错误给出安装目录中旧 CLI 的恢复命令并警告
    /// 不要用制品 CLI；原始原因保留在错误链末尾。
    #[test]
    fn stopped_before_install_names_the_installed_cli() {
        let failure = stopped_before_install(
            anyhow::anyhow!("pg_dump 失败").context("创建停机备份失败"),
            Path::new("/opt/auction/bin"),
        );
        assert!(!failure.recorded);
        let text = format!("{:#}", failure.error);
        assert!(text.contains("/opt/auction/bin/auction-cli service start"), "{text}");
        assert!(text.contains("不要用制品 CLI") && text.contains("迁移尚未执行"), "{text}");
        assert!(text.ends_with("创建停机备份失败: pg_dump 失败"), "{text}");
    }
}
