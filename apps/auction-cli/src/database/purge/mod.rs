//! 保留清理执行命令：读取一次新鲜的只读演练回执，派生逐数据族批准令牌；预览只打印
//! 计划与令牌，执行时先完成全部预校验与文件族清理，再按批准上限分批删除数据库候选，
//! 最后写入不可覆盖的私有回执。任何删除之前都必须通过政策、令牌、计数与目标身份检查。
//!
//! 不变量：文件删除不可撤销，删过文件的执行一定留下回执。删除文件前要求演练回执还剩
//! [`dry_run::FILESYSTEM_PURGE_MIN_HEADROOM_SECONDS`] 秒有效期；文件已删而数据库执行仍返回错误时，
//! 先写 `outcome=failed` 的本地回执再返回。

use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::Context;
use auction_storage::{
    RetentionPurgeApproval, RetentionPurgeFilesystemArtifactRef, RetentionPurgeOutcome,
    RetentionPurgeTableApproval,
};
use auction_time::Timestamp;
use sha2::{Digest, Sha256};

use self::dry_run::DryRunReceipt;
use crate::config_dir::ConfigDir;
use crate::support::digest::sha256_bytes;
use crate::support::files;

mod execute;
pub(crate) use execute::run;
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
    filesystem_root_identity_sha256: Option<String>,
    approval_token: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct FilesystemPurgeArtifact {
    format: &'static str,
    run_id: uuid::Uuid,
    dry_run_run_id: uuid::Uuid,
    database_name: String,
    dataset_family: String,
    approval_token_sha256: String,
    policy_version: i64,
    policy_sha256: String,
    filesystem_root_identity_sha256: String,
    approved_max_files: u64,
    completed_at: Timestamp,
    outcome: RetentionPurgeOutcome,
    directory_present: bool,
    examined_files: u64,
    deleted_files: u64,
    deleted_bytes: u64,
    retained_files: u64,
    write_unknown_held_files: u64,
    write_unknown_over_hold_window_files: u64,
    unexpected_entries: u64,
    limit_reached: bool,
    needs_follow_up: bool,
    failure_code: Option<String>,
}

#[derive(Debug, Clone)]
struct FilesystemPurgeEvidence {
    artifact: FilesystemPurgeArtifact,
    artifact_path: Option<PathBuf>,
    sha256: Option<String>,
}

#[derive(Debug, Clone)]
struct WrittenFilesystemPurgeArtifact {
    evidence: FilesystemPurgeEvidence,
    reference: RetentionPurgeFilesystemArtifactRef,
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
        let filesystem_root_identity_sha256 = (family.retention_kind == "filesystem")
            .then(|| receipt.filesystem_retention.root_identity_sha256.clone());
        let table_binding = tables
            .iter()
            .map(|table| (table.table.clone(), table.candidate_rows))
            .collect::<Vec<_>>();
        let approval_token = auction_storage::retention_purge_approval_token(
            auction_storage::RetentionPurgeApprovalTokenInput {
                database_name: database,
                dry_run_run_id: receipt.database_retention.run_id,
                policy_sha256,
                dataset_family: &family.dataset_family,
                filesystem_root_identity_sha256: filesystem_root_identity_sha256.as_deref(),
                candidate_rows: family.candidate_rows,
                candidate_bytes,
                tables: &table_binding,
            },
        );
        plans.push(FamilyPlan {
            dataset_family: family.dataset_family.clone(),
            retention_kind: family.retention_kind.clone(),
            candidate_rows: family.candidate_rows,
            candidate_bytes,
            tables,
            approval_token,
            filesystem_root_identity_sha256,
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
        auction_storage::RetentionPurgeApprovalTokenInput {
            database_name: database,
            dry_run_run_id: receipt.database_retention.run_id,
            policy_sha256,
            dataset_family: &filesystem.dataset_family,
            filesystem_root_identity_sha256: Some(&filesystem.root_identity_sha256),
            candidate_rows,
            candidate_bytes: Some(filesystem.candidate_bytes),
            tables: &[],
        },
    );
    plans.push(FamilyPlan {
        dataset_family: filesystem.dataset_family.clone(),
        retention_kind: "filesystem".to_owned(),
        candidate_rows,
        candidate_bytes: Some(filesystem.candidate_bytes),
        filesystem_root_identity_sha256: Some(filesystem.root_identity_sha256.clone()),
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
            filesystem_root_identity_sha256: plan.filesystem_root_identity_sha256.clone(),
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

/// Stable identity of the configured diagnostics root; never emits the configured path.
pub(super) fn filesystem_root_identity_sha256(path: &Path) -> anyhow::Result<String> {
    if !path.is_absolute()
        || path.components().any(|component| matches!(component, std::path::Component::ParentDir))
    {
        anyhow::bail!("邮件诊断目录身份路径无效");
    }
    let mut identity = b"auction-retention-filesystem-root/1\0".to_vec();
    identity.extend_from_slice(path.as_os_str().as_encoded_bytes());
    identity.push(0);
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path);
    match directory {
        Ok(directory) => {
            let metadata = directory.metadata().context("读取邮件诊断目录身份失败")?;
            if !metadata.is_dir() {
                anyhow::bail!("邮件诊断根不是目录");
            }
            identity.push(1);
            identity.extend_from_slice(&metadata.dev().to_be_bytes());
            identity.extend_from_slice(&metadata.ino().to_be_bytes());
            identity.extend_from_slice(&metadata.uid().to_be_bytes());
            identity.extend_from_slice(&metadata.gid().to_be_bytes());
            identity.extend_from_slice(&(metadata.mode() & 0o7777).to_be_bytes());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => identity.push(0),
        Err(error) => return Err(error).context("打开邮件诊断目录身份失败"),
    }
    Ok(sha256_bytes(&identity))
}

fn run_filesystem_purge(
    config: &auction_config::AppConfig,
    plans: &[FamilyPlan],
) -> anyhow::Result<auction_integrations::mooglemail::MailDiagnosticPurgeOutcome> {
    let plan = plans
        .iter()
        .find(|plan| plan.dataset_family == mail_family())
        .context("已批准的文件数据族不在演练回执中")?;
    let (ordinary_days, hold_days) = super::mail_diagnostic_windows()?;
    let max_files = u64::try_from(plan.candidate_rows).context("候选文件数无效")?;
    let max_bytes = plan.candidate_bytes.context("文件族批准缺少字节上限")?;
    let directory = &config.worker.mooglemail.diagnostics.directory;
    let snapshot = auction_integrations::mooglemail::mail_diagnostic_retention_snapshot(
        directory,
        super::days_to_duration(ordinary_days)?,
        super::days_to_duration(hold_days)?,
        SystemTime::now(),
    )
    .map_err(|error| anyhow::anyhow!("删除前重新统计加密诊断文件失败：{}", error.as_str()))?;
    anyhow::ensure!(
        snapshot.ordinary_expired_files == max_files
            && snapshot.ordinary_expired_bytes == max_bytes,
        "当前文件候选数或字节数与批准的演练回执不一致；未删除任何文件"
    );
    let outcome = auction_integrations::mooglemail::mail_diagnostic_purge_ordinary(
        directory,
        super::days_to_duration(ordinary_days)?,
        super::days_to_duration(hold_days)?,
        SystemTime::now(),
        max_files,
        max_bytes,
    )
    .map_err(|error| anyhow::anyhow!("清理加密邮件诊断文件失败：{}", error.as_str()))?;
    Ok(outcome)
}

fn build_filesystem_purge_artifact(
    execution_id: uuid::Uuid,
    dry_run: &DryRunReceipt,
    database: &str,
    policy_version: i64,
    policy_sha256: &str,
    plan: &FamilyPlan,
    mut outcome: auction_integrations::mooglemail::MailDiagnosticPurgeOutcome,
) -> anyhow::Result<FilesystemPurgeArtifact> {
    let approved_max_files = u64::try_from(plan.candidate_rows).context("批准的文件数无效")?;
    let approved_max_bytes = plan.candidate_bytes.context("文件族批准缺少字节上限")?;
    let exceeded_approval =
        outcome.deleted_files > approved_max_files || outcome.deleted_bytes > approved_max_bytes;
    if exceeded_approval {
        outcome.failure_code.get_or_insert("evidence_file_changed");
        outcome.needs_follow_up = true;
    }
    let filesystem_root_identity_sha256 = plan
        .filesystem_root_identity_sha256
        .as_ref()
        .context("批准的文件数据族缺少根身份摘要")?
        .clone();
    let artifact = FilesystemPurgeArtifact {
        format: "auction-retention-filesystem-purge-artifact.v1",
        run_id: execution_id,
        dry_run_run_id: dry_run.run_id,
        database_name: database.to_owned(),
        dataset_family: plan.dataset_family.clone(),
        approval_token_sha256: sha256_bytes(plan.approval_token.as_bytes()),
        policy_version,
        policy_sha256: policy_sha256.to_owned(),
        filesystem_root_identity_sha256,
        approved_max_files,
        completed_at: Timestamp::now(),
        outcome: if outcome.failure_code.is_some() {
            RetentionPurgeOutcome::Failed
        } else if outcome.limit_reached {
            RetentionPurgeOutcome::PartialLimitReached
        } else {
            RetentionPurgeOutcome::Complete
        },
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
        failure_code: outcome.failure_code.map(str::to_owned),
    };
    Ok(artifact)
}

fn publish_filesystem_purge_artifact(
    receipt_path: &Path,
    artifact: &FilesystemPurgeArtifact,
) -> anyhow::Result<WrittenFilesystemPurgeArtifact> {
    let bytes = serde_json::to_vec(&artifact).context("序列化文件清理证据失败")?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let sha256 = sha256_bytes(&bytes);
    let directory_path =
        receipt_path.with_file_name(format!(".filesystem-purge-{}", artifact.run_id));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory_path)
        .with_context(|| format!("创建文件清理证据目录失败：{}", directory_path.display()))?;
    let directory = files::PrivateDirectory::open_existing(&directory_path)?;
    directory.write("artifact.json", &bytes, false)?;
    let path = directory_path.join("artifact.json");
    Ok(WrittenFilesystemPurgeArtifact {
        evidence: FilesystemPurgeEvidence {
            artifact: artifact.clone(),
            artifact_path: Some(path.clone()),
            sha256: Some(sha256),
        },
        reference: RetentionPurgeFilesystemArtifactRef { path, sha256: digest },
    })
}

#[cfg(test)]
mod tests;
