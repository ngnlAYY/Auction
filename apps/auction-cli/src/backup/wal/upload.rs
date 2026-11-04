//! 暂存 WAL 上传器：把 `<段名>.zst.age` 上传到 R2 的独立 `wal/` 前缀，完整校验后删除本地副本。
//!
//! 独立于 PostgreSQL 运行（systemd timer，上传账号）；归档命令只写本地暂存。对象已存在且
//! 长度与 SHA-256 一致时视为成功（bucket lock 下重传会返回 409），不覆盖、不改名。

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use auction_config::AppConfig;
use auction_integrations::{BackupObject, BackupObjectStatus, R2BackupStore};
use serde::Serialize;
use tokio::time::timeout;

use super::model::{Staged, require_staging_dir, staged};
use crate::{
    app::{print_json, progress},
    backup::manifest::sha256_file,
    deployment::ConfigDir,
    support::files,
};

/// 上传器互斥锁；与归档进程共用的暂存目录内固定文件名。
const UPLOAD_LOCK: &str = "wal-upload.lock";
/// R2 中 WAL 的独立前缀；与日/月备份和基础备份分开配置生命周期。
const WAL_PREFIX: &str = "wal";

#[derive(Debug, Serialize)]
struct Report {
    outcome: UploadOutcome,
    staging_dir: PathBuf,
    uploaded: Vec<Uploaded>,
    already_present: Vec<Uploaded>,
    failed: Vec<Failed>,
    invalid: Vec<Failed>,
    incomplete: Vec<String>,
    remaining: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum UploadOutcome {
    Completed,
    Failed,
    Skipped,
}

impl UploadOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Serialize)]
struct Uploaded {
    segment_name: String,
    key: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct Failed {
    segment_name: String,
    reason: String,
}

/// 上传全部可用的暂存段；任何失败或超时都以非零退出，本地文件保留到校验通过为止。
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
    if !config.backup.offsite.enabled {
        progress("wal_upload status=skipped reason=offsite_disabled");
        print_json(&Report {
            outcome: UploadOutcome::Skipped,
            staging_dir: staging_dir.to_path_buf(),
            uploaded: Vec::new(),
            already_present: Vec::new(),
            failed: Vec::new(),
            invalid: Vec::new(),
            incomplete: Vec::new(),
            remaining: 0,
        })?;
        return Ok(());
    }
    let store = store(&config)?;
    let wal_prefix = object_key(&config.backup.offsite.prefix, WAL_PREFIX);
    let scan = staged(staging_dir)?;
    let total = scan.ready.len();
    let mut report = Report {
        outcome: UploadOutcome::Completed,
        staging_dir: staging_dir.to_path_buf(),
        uploaded: Vec::new(),
        already_present: Vec::new(),
        failed: Vec::new(),
        invalid: scan
            .invalid
            .into_iter()
            .map(|entry| Failed { segment_name: entry.segment_name, reason: entry.reason })
            .collect(),
        incomplete: scan.incomplete,
        remaining: 0,
    };
    let budget = Duration::from_secs(budget_seconds.clamp(1, 3600));
    let mut processed = 0_usize;
    let outcome = timeout(budget, async {
        for item in scan.ready {
            progress(format_args!("wal_upload segment={}", item.segment_name));
            match upload_one(&store, &wal_prefix, &item).await {
                Ok(object) => {
                    report.record(&item.segment_name, object);
                    // 远端已完整校验；先删密文再删 manifest，崩溃只遗留极小的 manifest。
                    if let Err(error) = remove_local(&item) {
                        report.failed.push(Failed {
                            segment_name: item.segment_name.clone(),
                            reason: format!("{error:#}"),
                        });
                    }
                }
                Err(error) => report.failed.push(Failed {
                    segment_name: item.segment_name.clone(),
                    reason: format!("{error:#}"),
                }),
            }
            processed += 1;
        }
    })
    .await;
    report.remaining = total.saturating_sub(processed);
    if outcome.is_err() {
        report
            .failed
            .push(Failed {
                segment_name: "-".to_owned(), reason: "上传总预算耗尽".to_owned()
            });
    }
    if !report.failed.is_empty() || !report.invalid.is_empty() || report.remaining > 0 {
        report.outcome = UploadOutcome::Failed;
    }
    progress(format_args!(
        "wal_upload outcome={} uploaded={} already_present={} failed={} invalid={} remaining={}",
        report.outcome.as_str(),
        report.uploaded.len(),
        report.already_present.len(),
        report.failed.len(),
        report.invalid.len(),
        report.remaining
    ));
    print_json(&report)?;
    if report.outcome != UploadOutcome::Completed {
        anyhow::bail!("WAL 上传未全部完成；本地暂存保留，按报告逐项核对");
    }
    Ok(())
}

impl Report {
    fn record(&mut self, segment_name: &str, object: BackupObject) {
        let entry = Uploaded {
            segment_name: segment_name.to_owned(),
            key: object.key,
            bytes: object.bytes,
            sha256: object.sha256,
        };
        match object.status {
            BackupObjectStatus::Uploaded => self.uploaded.push(entry),
            BackupObjectStatus::AlreadyPresent => self.already_present.push(entry),
        }
    }
}

fn store(config: &AppConfig) -> anyhow::Result<R2BackupStore> {
    let offsite = &config.backup.offsite;
    Ok(R2BackupStore::new(
        &offsite.endpoint,
        &offsite.bucket,
        config.backup_r2_access_key_id().context("缺少 R2 access key")?,
        config.backup_r2_secret_access_key().context("缺少 R2 secret")?,
    )?)
}

/// 上传单个暂存段：本地长度与 manifest 一致、摘要由本机计算，远端只做长度与完整回读校验。
async fn upload_one(
    store: &R2BackupStore,
    wal_prefix: &str,
    item: &Staged,
) -> anyhow::Result<BackupObject> {
    let cipher_bytes = fs::symlink_metadata(&item.cipher)
        .with_context(|| format!("读取 WAL 密文失败：{}", item.cipher.display()))?
        .len();
    if cipher_bytes != item.record.cipher_bytes {
        anyhow::bail!("WAL 密文长度与 manifest 不一致");
    }
    let sha256 = sha256_file(&item.cipher).await?;
    let file = files::open_regular(&item.cipher)?;
    let key = format!("{wal_prefix}/{}", file_name(&item.cipher)?);
    let object = store.upload_verified(&key, file, cipher_bytes, &sha256).await?;
    Ok(object)
}

fn remove_local(item: &Staged) -> anyhow::Result<()> {
    fs::remove_file(&item.cipher)
        .with_context(|| format!("删除已上传的 WAL 密文失败：{}", item.cipher.display()))?;
    fs::remove_file(&item.manifest)
        .with_context(|| format!("删除已上传的 WAL manifest 失败：{}", item.manifest.display()))?;
    let directory = item.cipher.parent().context("WAL 密文缺少父目录")?;
    fs::File::open(directory)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("同步 WAL 暂存目录失败：{}", directory.display()))
}

fn file_name(path: &Path) -> anyhow::Result<&str> {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("WAL 密文文件名无效：{}", path.display()))
}

fn object_key(prefix: &str, tail: &str) -> String {
    if prefix.is_empty() { tail.to_owned() } else { format!("{prefix}/{tail}") }
}

#[cfg(test)]
mod tests {
    use super::{WAL_PREFIX, object_key};

    #[test]
    fn object_keys_join_prefix_without_leading_slash() {
        assert_eq!(object_key("", WAL_PREFIX), "wal");
        assert_eq!(object_key("auction", WAL_PREFIX), "auction/wal");
        assert_eq!(
            format!("{}/{}", object_key("auction", WAL_PREFIX), "000000010000000000000001.zst.age"),
            "auction/wal/000000010000000000000001.zst.age"
        );
    }
}
