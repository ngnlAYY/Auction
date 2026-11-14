//! 暂存上传器：把 WAL 段上传到 R2 的 `wal/` 前缀，把每周基础备份上传到 `base/<备份 ID>/`；
//! 两侧都用长度与完整 SHA-256 校验，对象已存在且内容一致时视为成功（bucket lock 下重传会
//! 返回 409），不覆盖、不改名。WAL 校验后删除本地副本；基础备份按本地保留策略保留最近
//! `BASE_RETENTION_KEEP` 份（与 R2 的 21 天生命周期对齐，决策 49）。

use std::{path::Path, time::Duration};

use tokio::time::timeout;

use super::model::{
    BASE_RETENTION_KEEP, WAL_UPLOAD_FORMAT, WalUploadReceipt, require_staging_dir, retention_plan,
    staged,
};
use crate::{
    app::{print_json, progress},
    config_dir::ConfigDir,
    support::files,
};

/// 上传器互斥锁；与归档进程共用的暂存目录内固定文件名。
const UPLOAD_LOCK: &str = "wal-upload.lock";
/// R2 中 WAL 的独立前缀；与日/月备份和基础备份分开配置生命周期。
const WAL_PREFIX: &str = "wal";
/// R2 中每周基础备份的独立前缀；每个备份一个目录。
const BASE_PREFIX: &str = "base";
/// 基础备份上传记录的格式名；字段只追加，旧记录继续可读。
const BASE_UPLOAD_FORMAT: &str = "auction-base-upload.v1";

/// 上传全部可用的暂存项；任何失败或超时都以非零退出，本地文件保留到校验通过为止。
pub(super) async fn run(
    config_dir: &ConfigDir,
    staging_dir: &Path,
    budget_seconds: u64,
) -> anyhow::Result<()> {
    require_staging_dir(staging_dir)?;
    let _lock =
        files::lock_file(&staging_dir.join(UPLOAD_LOCK), "另一个 WAL 上传器正在使用该暂存目录")?;
    let config = config_dir.load_worker()?;
    config.validate_backup()?;
    let scan = staged(staging_dir)?;
    let mut report = Report {
        outcome: UploadOutcome::Completed,
        staging_dir: staging_dir.to_path_buf(),
        uploaded: Vec::new(),
        already_present: Vec::new(),
        failed: Vec::new(),
        invalid: scan
            .invalid
            .into_iter()
            .map(|entry| Failed { name: entry.name, reason: entry.reason })
            .collect(),
        incomplete: scan.incomplete,
        remaining: 0,
        base: BaseReport::default(),
    };
    if !config.backup.offsite.enabled {
        progress("wal_upload status=skipped reason=offsite_disabled");
        // 异机副本关闭时暂存目录仍是唯一副本；本地保留策略照常生效，避免无界增长。
        let plan = retention_plan(scan.base, BASE_RETENTION_KEEP, &[]);
        prune(staging_dir, plan, &mut report.base);
        report.outcome = if report.base.failed.is_empty() {
            UploadOutcome::Skipped
        } else {
            UploadOutcome::Failed
        };
        print_json(&report)?;
        if report.outcome == UploadOutcome::Failed {
            anyhow::bail!("基础备份本地保留清理失败；按报告逐项核对");
        }
        return Ok(());
    }
    let store = store(&config)?;
    let wal_prefix = object_key(&config.backup.offsite.prefix, WAL_PREFIX);
    let base_prefix = object_key(&config.backup.offsite.prefix, BASE_PREFIX);
    let destination = base_destination(&config, &base_prefix);
    let total = scan.ready.len();
    let budget = Duration::from_secs(budget_seconds.clamp(1, 3600));
    let mut processed = 0_usize;
    let mut failed_base = Vec::new();
    let outcome = timeout(budget, async {
        for item in &scan.ready {
            progress(format_args!("wal_upload segment={}", item.segment_name));
            match upload_one(&store, &wal_prefix, item).await {
                Ok(object) => {
                    // 远端已完整校验。先写持久上传证据，再删密文/manifest；否则 PostgreSQL 在
                    // 重启后重试同一段时会因本地证据消失而重新加密，随机 age 密文与远端对象
                    // 摘要不一致，导致永久失败。
                    let receipt = WalUploadReceipt {
                        format: WAL_UPLOAD_FORMAT.to_owned(),
                        segment_name: item.segment_name.clone(),
                        source_bytes: item.record.source_bytes,
                        source_mtime_ns: item.record.source_mtime_ns,
                        cipher_bytes: object.bytes,
                        cipher_sha256: object.sha256.clone(),
                        remote_key: object.key.clone(),
                        uploaded_unix_seconds: chrono::Utc::now().timestamp().max(1) as u64,
                    };
                    if let Err(error) = write_wal_upload_receipt(staging_dir, &receipt) {
                        report.failed.push(Failed {
                            name: item.segment_name.clone(),
                            reason: format!("写入 WAL 上传证据失败：{error:#}"),
                        });
                    } else {
                        report.record(&item.segment_name, object);
                        if let Err(error) = remove_local(item) {
                            report.failed.push(Failed {
                                name: item.segment_name.clone(),
                                reason: format!("{error:#}"),
                            });
                        }
                    }
                }
                Err(error) => report
                    .failed
                    .push(Failed { name: item.segment_name.clone(), reason: format!("{error:#}") }),
            }
            processed += 1;
        }
        for item in &scan.base {
            if item.uploaded && item.receipt_destination.as_deref() == Some(destination.as_str()) {
                // 本机证据表明这组密文与 manifest 已在同一目标完成远端校验，不再重复回读。
                report.base.already_verified.push(item.backup_id.clone());
                continue;
            }
            progress(format_args!("wal_upload base_backup={}", item.backup_id));
            match upload_base(&store, &base_prefix, item).await {
                Ok(uploaded) => {
                    let completion_sha256 = uploaded.completion.sha256.clone();
                    report.base.record(uploaded);
                    if let Err(error) = write_receipt(
                        staging_dir,
                        &item.backup_id,
                        &destination,
                        &completion_sha256,
                    ) {
                        report.base.failed.push(Failed {
                            name: item.backup_id.clone(),
                            reason: format!("写入本机上传证据失败：{error:#}"),
                        });
                    }
                }
                Err(error) => {
                    failed_base.push(item.backup_id.clone());
                    report.base.failed.push(Failed {
                        name: item.backup_id.clone(),
                        reason: format!("{error:#}"),
                    });
                }
            }
        }
        // 上传失败的旧基础备份也保留：不因为另一份新备份上传成功就删掉唯一本地副本。
        let plan = retention_plan(scan.base, BASE_RETENTION_KEEP, &failed_base);
        prune(staging_dir, plan, &mut report.base);
    })
    .await;
    report.remaining = total.saturating_sub(processed);
    if outcome.is_err() {
        let failure = Failed { name: "-".to_owned(), reason: "上传总预算耗尽".to_owned() };
        report.failed.push(failure.clone());
        report.base.failed.push(failure);
    }
    if !report.failed.is_empty()
        || !report.invalid.is_empty()
        || !report.base.failed.is_empty()
        || report.remaining > 0
    {
        report.outcome = UploadOutcome::Failed;
    }
    progress(format_args!(
        "wal_upload outcome={} uploaded={} already_present={} failed={} invalid={} remaining={} base_uploaded={} base_failed={} base_removed={}",
        report.outcome.as_str(),
        report.uploaded.len(),
        report.already_present.len(),
        report.failed.len(),
        report.invalid.len(),
        report.remaining,
        report.base.uploaded.len() + report.base.already_present.len(),
        report.base.failed.len(),
        report.base.removed.len()
    ));
    print_json(&report)?;
    if report.outcome != UploadOutcome::Completed {
        anyhow::bail!("WAL/基础备份上传未全部完成；本地暂存保留，按报告逐项核对");
    }
    Ok(())
}

mod report;
#[cfg(test)]
mod tests;
mod transfer;
use report::*;
use transfer::*;
