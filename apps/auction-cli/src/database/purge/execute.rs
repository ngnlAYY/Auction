//! 保留清理的预览、批准执行与回执发布。

use anyhow::Context;
use auction_storage::{
    RETENTION_PURGE_MAX_BATCH_SIZE, RETENTION_PURGE_MIN_BATCH_SIZE, RETENTION_PURGE_POLICY_VERSION,
    RETENTION_PURGE_RECEIPT_FORMAT, RetentionPurgeOutcome, RetentionPurgeRequest,
};
use auction_time::Timestamp;

use super::dry_run::{load_receipt, require_receipt_headroom, validate_receipt};
use super::failure::execution_failed;
use super::{
    FilesystemPurgeEvidence, Options, build_filesystem_purge_artifact, build_plans,
    filesystem_root_identity_sha256, mail_family, parse_approvals,
    publish_filesystem_purge_artifact, run_filesystem_purge,
};
use crate::app::print_json;
use crate::database::{confirmed_worker, with_store};
use crate::operations::operator_identity;
use crate::support::files;
pub(crate) async fn run(options: Options<'_>) -> anyhow::Result<()> {
    anyhow::ensure!(
        (RETENTION_PURGE_MIN_BATCH_SIZE..=RETENTION_PURGE_MAX_BATCH_SIZE)
            .contains(&options.batch_size),
        "批次大小必须在 {RETENTION_PURGE_MIN_BATCH_SIZE}..={RETENTION_PURGE_MAX_BATCH_SIZE} 之间"
    );
    let receipt = load_receipt(options.dry_run_receipt)?;
    let (policy_version, policy_sha256) =
        auction_storage::retention_policy_identity().context("编译制品内嵌保留政策无效")?;
    anyhow::ensure!(policy_version == RETENTION_PURGE_POLICY_VERSION, "编译制品的保留政策不是 v2");
    validate_receipt(&receipt, options.database, policy_version, &policy_sha256, Timestamp::now())?;
    let plans = build_plans(&receipt, options.database, &policy_sha256)?;

    if !options.execute {
        print_json(&serde_json::json!({
            "execute": false,
            "dry_run_receipt": options.dry_run_receipt,
            "dry_run_run_id": receipt.run_id,
            "dry_run_created_at": receipt.created_at,
            "database": receipt.database_retention.database_name,
            "policy_version": receipt.database_retention.policy_version,
            "policy_sha256": receipt.database_retention.policy_sha256,
            "batch_size": options.batch_size,
            "families": plans.iter().map(|plan| serde_json::json!({
                "dataset_family": plan.dataset_family,
                "retention_kind": plan.retention_kind,
                "candidate_rows": plan.candidate_rows,
                "candidate_bytes": plan.candidate_bytes,
                "tables": plan.tables.iter().map(|table| serde_json::json!({
                    "table": table.table,
                    "rule_ids": table.rule_ids,
                    "candidate_rows": table.candidate_rows,
                })).collect::<Vec<_>>(),
                "approval_token": plan.approval_token,
            })).collect::<Vec<_>>(),
            "next": "复核候选计数后，以 --execute --approve <family>=<approval_token> --confirm 'PURGE <database>' --receipt <file> 重新运行",
            "mutation_executed": false,
            "deletion_authorized": false,
        }))?;
        return Ok(());
    }

    let expected_confirm = format!("PURGE {}", options.database);
    anyhow::ensure!(
        options.confirm == Some(expected_confirm.as_str()),
        "确认串必须逐字为 '{expected_confirm}'"
    );
    let receipt_path = options.receipt.context("实际执行必须提供 --receipt")?;
    files::require_private_target(receipt_path)?;
    let approvals = parse_approvals(options.approvals, &plans)?;
    // 删除业务库行的审计必须能追溯到主机操作者；采集失败时在任何连接与删除之前拒绝。
    let operator = operator_identity::capture().context("采集操作者身份失败；未执行任何删除")?;

    let config = confirmed_worker(options.config_dir, options.database)?;
    let filesystem_plan = plans
        .iter()
        .find(|plan| plan.dataset_family == mail_family())
        .context("演练回执缺少文件数据族")?;
    let filesystem_execution_id = approvals
        .iter()
        .any(|approval| approval.dataset_family == mail_family())
        .then(uuid::Uuid::new_v4);
    if filesystem_execution_id.is_some() {
        let approved_root = filesystem_plan
            .filesystem_root_identity_sha256
            .as_deref()
            .context("批准的文件数据族缺少根身份摘要")?;
        let current_root =
            filesystem_root_identity_sha256(&config.worker.mooglemail.diagnostics.directory)?;
        anyhow::ensure!(
            current_root == approved_root,
            "邮件诊断目录身份与只读演练不一致；未执行任何删除，请重新演练"
        );
    }
    let (report, filesystem_artifact) = with_store(
        &config,
        auction_storage::DatabaseConnectionPurpose::CliRetentionPurge,
        async |store| {
            let mut request = RetentionPurgeRequest {
                database_name: receipt.database_retention.database_name.clone(),
                dry_run_run_id: receipt.database_retention.run_id,
                dry_run_collected_at: receipt.database_retention.collected_at,
                dry_run_created_at: receipt.created_at,
                policy_sha256: policy_sha256.clone(),
                batch_size: options.batch_size,
                approvals: approvals.clone(),
                filesystem_artifact: None,
                operator_identity: Some(operator.clone()),
            };
            store
                .preflight_retention_purge(&request)
                .await
                .context("保留清理预校验失败；未执行任何删除")?;
            let filesystem_artifact = if let Some(execution_id) = filesystem_execution_id {
                require_receipt_headroom(receipt.created_at, Timestamp::now())?;
                let expected_root = filesystem_plan
                    .filesystem_root_identity_sha256
                    .as_deref()
                    .context("批准的文件数据族缺少根身份摘要")?;
                let current_root =
                    filesystem_root_identity_sha256(&config.worker.mooglemail.diagnostics.directory)?;
                anyhow::ensure!(
                    current_root == expected_root,
                    "邮件诊断目录身份在删除前变化；未执行任何删除，请重新演练"
                );
                let approved_max_files =
                    u64::try_from(filesystem_plan.candidate_rows).context("批准的文件数无效")?;
                let approved_max_bytes =
                    filesystem_plan.candidate_bytes.context("文件族批准缺少字节上限")?;
                let outcome = run_filesystem_purge(&config, &plans)?;
                let exceeded_approval =
                    outcome.deleted_files > approved_max_files || outcome.deleted_bytes > approved_max_bytes;
                let artifact = build_filesystem_purge_artifact(
                    execution_id,
                    &receipt,
                    options.database,
                    policy_version,
                    &policy_sha256,
                    filesystem_plan,
                    outcome,
                )?;
                let published = match publish_filesystem_purge_artifact(receipt_path, &artifact) {
                    Ok(published) => published,
                    Err(write_error) => {
                        let deleted_files = artifact.deleted_files;
                        let evidence = FilesystemPurgeEvidence {
                            artifact,
                            artifact_path: None,
                            sha256: None,
                        };
                        let error = execution_failed(
                            options.dry_run_receipt,
                            &receipt,
                            receipt_path,
                            Some(evidence),
                            auction_storage::RetentionPurgeError::FilesystemInvalid,
                        );
                        return Err(error.context(format!(
                            "文件清理已删除 {deleted_files} 个文件，但执行证据发布失败：{write_error:#}"
                        )));
                    }
                };
                if exceeded_approval {
                    let deleted_files = artifact.deleted_files;
                    let deleted_bytes = artifact.deleted_bytes;
                    let error = execution_failed(
                        options.dry_run_receipt,
                        &receipt,
                        receipt_path,
                        Some(published.evidence),
                        auction_storage::RetentionPurgeError::FilesystemInvalid,
                    );
                    return Err(error.context(format!(
					"文件清理删除 {deleted_files} 个文件/{deleted_bytes} 字节，超过批准上限 {approved_max_files} 个文件/{approved_max_bytes} 字节；数据库清理未执行"
                    )));
                }
                Some(published)
            } else {
                None
            };
            request.filesystem_artifact =
                filesystem_artifact.as_ref().map(|artifact| artifact.reference.clone());
            let report = store.execute_retention_purge(request).await.map_err(|error| {
                execution_failed(
                    options.dry_run_receipt,
                    &receipt,
                    receipt_path,
                    filesystem_artifact.as_ref().map(|artifact| artifact.evidence.clone()),
                    error,
                )
            })?;
            Ok((report, filesystem_artifact))
        },
    )
    .await?;

    let receipt_document = serde_json::json!({
        "format": RETENTION_PURGE_RECEIPT_FORMAT,
        "created_at": Timestamp::now(),
        "dry_run_receipt": options.dry_run_receipt,
        "dry_run_run_id": receipt.run_id,
        "dry_run_created_at": receipt.created_at,
        "filesystem_artifact": filesystem_artifact.as_ref().map(|published| serde_json::json!({
            "path": published.evidence.artifact_path,
            "sha256": published.evidence.sha256,
        })),
        "execution": &report,
        "notes": [
            "回执只记录计数与安全分类，不包含行内容、凭据或令牌",
            "deleted_row_bytes 是删除时 pg_column_size 的行载荷字节数，不是物理页面大小",
            "partial_limit_reached 表示批准上限内已删完但仍有候选；须重新只读演练并再次批准",
        ],
    });
    files::write_private(receipt_path, &serde_json::to_vec_pretty(&receipt_document)?)?;

    print_json(&serde_json::json!({
        "output": receipt_path,
        "run_id": report.run_id,
        "dry_run_run_id": report.dry_run_run_id,
        "database": report.database_name,
        "outcome": report.outcome,
        "failure_code": report.failure_code,
        "deleted_rows": report.deleted_rows,
        "deleted_row_bytes": report.deleted_row_bytes,
        "filesystem_deleted_files": report.filesystem.as_ref().map(|filesystem| filesystem.deleted_files),
        "filesystem_deleted_bytes": report.filesystem.as_ref().map(|filesystem| filesystem.deleted_bytes),
        "filesystem_artifact": filesystem_artifact.as_ref().map(|published| serde_json::json!({
            "path": published.evidence.artifact_path,
            "sha256": published.evidence.sha256,
        })),
        "audit_id": report.audit_id,
        "mutation_executed": report.mutation_executed,
    }))?;
    if report.outcome == RetentionPurgeOutcome::Failed {
        anyhow::bail!(
            "保留清理未完成（{}）；回执已写入 {}",
            report.failure_code.unwrap_or("unknown"),
            receipt_path.display()
        );
    }
    Ok(())
}
