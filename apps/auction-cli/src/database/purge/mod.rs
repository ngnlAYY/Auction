//! 保留清理执行命令：读取一次新鲜的只读演练回执，派生逐数据族批准令牌；预览只打印
//! 计划与令牌，执行时先完成全部预校验与文件族清理，再按批准上限分批删除数据库候选，
//! 最后写入不可覆盖的私有回执。任何删除之前都必须通过政策、令牌、计数与目标身份检查。
//!
//! 不变量：文件删除不可撤销，删过文件的执行一定留下回执。删除文件前要求演练回执还剩
//! [`dry_run::FILESYSTEM_PURGE_MIN_HEADROOM_SECONDS`] 秒有效期；文件已删而数据库执行仍返回错误时，
//! 先写 `outcome=failed` 的本地回执再返回。

use std::{path::Path, time::SystemTime};

use anyhow::Context;
use auction_storage::{
    RETENTION_PURGE_MAX_BATCH_SIZE, RETENTION_PURGE_MIN_BATCH_SIZE, RETENTION_PURGE_POLICY_VERSION,
    RETENTION_PURGE_RECEIPT_FORMAT, RetentionPurgeApproval, RetentionPurgeFilesystemResult,
    RetentionPurgeOutcome, RetentionPurgeRequest, RetentionPurgeTableApproval,
};
use chrono::Utc;

use self::dry_run::{DryRunReceipt, load_receipt, require_receipt_headroom, validate_receipt};
use self::failure::execution_failed;
use crate::app::print_json;
use crate::config_dir::ConfigDir;
use crate::support::files;

mod dry_run;
mod failure;

pub(crate) struct Options<'a> {
    pub(crate) config_dir: &'a ConfigDir,
    pub(crate) database: &'a str,
    pub(crate) dry_run_receipt: &'a Path,
    pub(crate) approvals: &'a [String],
    pub(crate) execute: bool,
    pub(crate) confirm: Option<&'a str>,
    pub(crate) receipt: Option<&'a Path>,
    pub(crate) batch_size: u32,
}

struct FamilyPlan {
    dataset_family: String,
    retention_kind: String,
    candidate_rows: i64,
    candidate_bytes: Option<u64>,
    tables: Vec<RetentionPurgeTableApproval>,
    approval_token: String,
}

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
    validate_receipt(&receipt, options.database, policy_version, &policy_sha256, Utc::now())?;
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

    let config = super::confirmed_worker(options.config_dir, options.database)?;
    let report = super::with_store(
        &config,
        auction_storage::DatabaseConnectionPurpose::CliRetentionPurge,
        async |store| {
            // 先用零值占位结果完成包含目标身份与迁移前缀在内的全部预校验；预校验失败时
            // 不清理任何文件、不执行任何 DELETE。
            let placeholder_filesystem = approvals
                .iter()
                .any(|approval| approval.dataset_family == mail_family())
                .then(zero_filesystem_result);
            let mut request = RetentionPurgeRequest {
                database_name: receipt.database_retention.database_name.clone(),
                dry_run_run_id: receipt.database_retention.run_id,
                dry_run_collected_at: receipt.database_retention.collected_at,
                dry_run_created_at: receipt.created_at,
                policy_sha256: policy_sha256.clone(),
                batch_size: options.batch_size,
                approvals: approvals.clone(),
                filesystem: placeholder_filesystem,
            };
            store
                .preflight_retention_purge(&request)
                .await
                .context("保留清理预校验失败；未执行任何删除")?;
            if request.filesystem.is_some() {
                require_receipt_headroom(receipt.created_at, Utc::now())?;
                request.filesystem = Some(run_filesystem_purge(&config, &plans)?);
            }
            let filesystem = request.filesystem.clone();
            store.execute_retention_purge(request).await.map_err(|error| {
                execution_failed(options.dry_run_receipt, &receipt, receipt_path, filesystem, error)
            })
        },
    )
    .await?;

    let receipt_document = serde_json::json!({
        "format": RETENTION_PURGE_RECEIPT_FORMAT,
        "created_at": Utc::now(),
        "dry_run_receipt": options.dry_run_receipt,
        "dry_run_run_id": receipt.run_id,
        "dry_run_created_at": receipt.created_at,
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

fn build_plans(
    receipt: &DryRunReceipt,
    database: &str,
    policy_sha256: &str,
) -> anyhow::Result<Vec<FamilyPlan>> {
    let mut plans = Vec::new();
    let mut seen_families = std::collections::BTreeSet::new();
    let mut seen_tables = std::collections::BTreeSet::new();
    for family in &receipt.database_retention.families {
        anyhow::ensure!(
            seen_families.insert(family.dataset_family.clone()),
            "只读演练回执重复数据族：{}",
            family.dataset_family
        );
        anyhow::ensure!(family.candidate_rows >= 0, "候选行数不能为负");
        let (candidate_bytes, tables) = match family.retention_kind.as_str() {
            "database_rows" => {
                anyhow::ensure!(!family.tables.is_empty(), "数据库数据族缺少表");
                let mut tables = Vec::with_capacity(family.tables.len());
                let mut sum = 0_i64;
                for table in &family.tables {
                    anyhow::ensure!(
                        seen_tables.insert(table.table.clone()),
                        "只读演练回执重复表：{}",
                        table.table
                    );
                    anyhow::ensure!(
                        table.candidate_rows >= 0 && !table.rule_ids.is_empty(),
                        "表 {} 的候选计数或规则无效",
                        table.table
                    );
                    sum = sum.checked_add(table.candidate_rows).context("候选行数溢出")?;
                    tables.push(RetentionPurgeTableApproval {
                        table: table.table.clone(),
                        rule_ids: table.rule_ids.clone(),
                        candidate_rows: table.candidate_rows,
                    });
                }
                anyhow::ensure!(sum == family.candidate_rows, "数据族与表级候选计数不一致");
                (None, tables)
            }
            "filesystem" => {
                anyhow::ensure!(family.tables.is_empty(), "文件数据族不能包含表");
                let filesystem = &receipt.filesystem_retention;
                anyhow::ensure!(
                    filesystem.dataset_family == family.dataset_family,
                    "文件数据族身份不一致"
                );
                anyhow::ensure!(
                    i64::try_from(filesystem.candidate_files).ok() == Some(family.candidate_rows),
                    "文件数据族的候选文件数不一致"
                );
                (Some(filesystem.candidate_bytes), Vec::new())
            }
            other => anyhow::bail!("不支持的保留类型：{other}"),
        };
        let table_binding = tables
            .iter()
            .map(|table| (table.table.clone(), table.candidate_rows))
            .collect::<Vec<_>>();
        let approval_token = auction_storage::retention_purge_approval_token(
            database,
            receipt.database_retention.run_id,
            policy_sha256,
            &family.dataset_family,
            family.candidate_rows,
            candidate_bytes,
            &table_binding,
        );
        plans.push(FamilyPlan {
            dataset_family: family.dataset_family.clone(),
            retention_kind: family.retention_kind.clone(),
            candidate_rows: family.candidate_rows,
            candidate_bytes,
            tables,
            approval_token,
        });
    }
    // 文件族不在数据库报告内，由只读演练的独立快照给出候选文件数与字节数。
    let filesystem = &receipt.filesystem_retention;
    anyhow::ensure!(
        !seen_families.contains(&filesystem.dataset_family),
        "只读演练回执重复数据族：{}",
        filesystem.dataset_family
    );
    let candidate_rows = i64::try_from(filesystem.candidate_files).context("候选文件数超出范围")?;
    anyhow::ensure!(candidate_rows >= 0, "候选文件数不能为负");
    let approval_token = auction_storage::retention_purge_approval_token(
        database,
        receipt.database_retention.run_id,
        policy_sha256,
        &filesystem.dataset_family,
        candidate_rows,
        Some(filesystem.candidate_bytes),
        &[],
    );
    plans.push(FamilyPlan {
        dataset_family: filesystem.dataset_family.clone(),
        retention_kind: "filesystem".to_owned(),
        candidate_rows,
        candidate_bytes: Some(filesystem.candidate_bytes),
        tables: Vec::new(),
        approval_token,
    });
    anyhow::ensure!(!plans.is_empty(), "只读演练回执没有可执行的数据族");
    Ok(plans)
}

fn parse_approvals(
    specs: &[String],
    plans: &[FamilyPlan],
) -> anyhow::Result<Vec<RetentionPurgeApproval>> {
    anyhow::ensure!(!specs.is_empty(), "实际执行必须至少批准一个数据族");
    let mut approvals = Vec::with_capacity(specs.len());
    let mut seen = std::collections::BTreeSet::new();
    for spec in specs {
        let (family, token) = spec
            .split_once('=')
            .with_context(|| format!("批准参数必须是 <family>=<token>：{spec}"))?;
        anyhow::ensure!(seen.insert(family.to_owned()), "数据族重复批准：{family}");
        let plan = plans
            .iter()
            .find(|plan| plan.dataset_family == family)
            .with_context(|| format!("只读演练回执没有数据族：{family}"))?;
        anyhow::ensure!(
            token == plan.approval_token,
            "数据族 {family} 的批准令牌与回执不匹配；重新预览以取得当前令牌"
        );
        approvals.push(RetentionPurgeApproval {
            dataset_family: plan.dataset_family.clone(),
            approval_token: token.to_owned(),
            candidate_rows: plan.candidate_rows,
            candidate_bytes: plan.candidate_bytes,
            tables: plan.tables.clone(),
        });
    }
    Ok(approvals)
}

fn mail_family() -> &'static str {
    "mail_diagnostic_files"
}

fn zero_filesystem_result() -> RetentionPurgeFilesystemResult {
    RetentionPurgeFilesystemResult {
        directory_present: false,
        examined_files: 0,
        deleted_files: 0,
        deleted_bytes: 0,
        retained_files: 0,
        write_unknown_held_files: 0,
        write_unknown_over_hold_window_files: 0,
        unexpected_entries: 0,
        limit_reached: false,
        needs_follow_up: false,
        failure_code: None,
    }
}

fn run_filesystem_purge(
    config: &auction_config::AppConfig,
    plans: &[FamilyPlan],
) -> anyhow::Result<RetentionPurgeFilesystemResult> {
    let plan = plans
        .iter()
        .find(|plan| plan.dataset_family == mail_family())
        .context("已批准的文件数据族不在演练回执中")?;
    let (ordinary_days, hold_days) = super::mail_diagnostic_windows()?;
    let max_files = u64::try_from(plan.candidate_rows).context("候选文件数无效")?;
    let outcome = auction_integrations::mooglemail::mail_diagnostic_purge_ordinary(
        &config.worker.mooglemail.diagnostics.directory,
        super::days_to_duration(ordinary_days)?,
        super::days_to_duration(hold_days)?,
        SystemTime::now(),
        max_files,
    )
    .map_err(|error| anyhow::anyhow!("清理加密邮件诊断文件失败：{}", error.as_str()))?;
    anyhow::ensure!(outcome.deleted_files <= max_files, "文件清理超过批准上限");
    Ok(RetentionPurgeFilesystemResult {
        directory_present: outcome.directory_present,
        examined_files: outcome.examined_files,
        deleted_files: outcome.deleted_files,
        deleted_bytes: outcome.deleted_bytes,
        retained_files: outcome.retained_files,
        write_unknown_held_files: outcome.write_unknown_held_files,
        write_unknown_over_hold_window_files: outcome.write_unknown_over_hold_window_files,
        unexpected_entries: outcome.unexpected_entries,
        limit_reached: outcome.limit_reached,
        needs_follow_up: outcome.needs_follow_up,
        failure_code: outcome.failure_code.map(std::borrow::ToOwned::to_owned),
    })
}

#[cfg(test)]
mod tests;
