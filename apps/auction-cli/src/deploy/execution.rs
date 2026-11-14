//! 持锁执行的部署主体：停机、替换、迁移与启动。

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

use super::receipts::{previous_source_commit, receipt_summary};
use super::verification::{installed_artifacts, verify_running_artifacts};
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
