//! 暂存上传器：把 WAL 段上传到 R2 的 `wal/` 前缀，把每周基础备份上传到 `base/<备份 ID>/`；
//! 两侧都用长度与完整 SHA-256 校验，对象已存在且内容一致时视为成功（bucket lock 下重传会
//! 返回 409），不覆盖、不改名。WAL 校验后删除本地副本；基础备份按本地保留策略保留最近
//! `BASE_RETENTION_KEEP` 份（与 R2 的 21 天生命周期对齐，决策 49）。

use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use auction_config::AppConfig;
use auction_integrations::{BackupObject, BackupObjectStatus, R2BackupStore};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use super::model::{
    BASE_RECEIPT_FORMAT, BASE_RECEIPT_SUFFIX, BASE_RETENTION_KEEP, BaseReceipt, BaseStaged,
    Retention, Staged, WAL_UPLOAD_FORMAT, WalUploadReceipt, base_receipt_path, require_staging_dir,
    retention_plan, staged, wal_upload_path,
};
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
/// R2 中每周基础备份的独立前缀；每个备份一个目录。
const BASE_PREFIX: &str = "base";
/// 基础备份上传记录的格式名；字段只追加，旧记录继续可读。
const BASE_UPLOAD_FORMAT: &str = "auction-base-upload.v1";

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
    /// 每周基础备份的独立结果；保留与删除只作用于本地暂存目录。
    base: BaseReport,
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

#[derive(Debug, Clone, Serialize)]
struct Failed {
    /// WAL 段名或基础备份 ID。
    name: String,
    reason: String,
}

#[derive(Debug, Default, Serialize)]
struct BaseReport {
    uploaded: Vec<BaseUploaded>,
    already_present: Vec<BaseUploaded>,
    /// 已有本机上传证据、本次未重复回读的备份 ID。
    already_verified: Vec<String>,
    failed: Vec<Failed>,
    /// 当前保留在暂存目录的基础备份 ID（最新在前）。
    retained: Vec<String>,
    /// 本次按保留策略删除的基础备份 ID。
    removed: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BaseUploaded {
    backup_id: String,
    directory: String,
    archive: BackupObject,
    manifest: BackupObject,
    completion: BackupObject,
}

/// 远端完成记录引用的对象摘要；不带本次上传状态，重试可生成完全相同字节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaseObject {
    key: String,
    bytes: u64,
    sha256: String,
}

/// 远端 `completed.json` 的内容；`created_unix_seconds` 取基础备份完成时刻，保证幂等。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaseUploadRecord {
    format: String,
    backup_id: String,
    created_unix_seconds: u64,
    archive: BaseObject,
    manifest: BaseObject,
}

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

impl BaseReport {
    fn record(&mut self, object: BaseUploaded) {
        // 以 archive 的上传状态代表整组结果：三份对象都在同一次运行中确认。
        match object.archive.status {
            BackupObjectStatus::Uploaded => self.uploaded.push(object),
            BackupObjectStatus::AlreadyPresent => self.already_present.push(object),
        }
    }
}

/// 基础备份上传证据的目标身份；只含 endpoint、bucket 与前缀，不含任何凭据。
fn base_destination(config: &AppConfig, base_prefix: &str) -> String {
    destination_identity(
        &config.backup.offsite.endpoint,
        &config.backup.offsite.bucket,
        base_prefix,
    )
}

fn destination_identity(endpoint: &str, bucket: &str, base_prefix: &str) -> String {
    // 使用非控制字符分隔，避免证据校验把合法身份误判为损坏。
    format!("{}\u{241f}{bucket}\u{241f}{base_prefix}", endpoint.trim_end_matches('/'))
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

/// 上传一组基础备份：先密文、再 manifest，最后发布字节固定的完成记录。
async fn upload_base(
    store: &R2BackupStore,
    base_prefix: &str,
    item: &BaseStaged,
) -> anyhow::Result<BaseUploaded> {
    let cipher_bytes = fs::symlink_metadata(&item.cipher)
        .with_context(|| format!("读取基础备份密文失败：{}", item.cipher.display()))?
        .len();
    if cipher_bytes != item.record.cipher_bytes {
        anyhow::bail!("基础备份密文长度与 manifest 不一致");
    }
    let sha256 = sha256_file(&item.cipher).await?;
    if sha256 != item.record.cipher_sha256 {
        anyhow::bail!("基础备份密文 SHA-256 与 manifest 不一致");
    }
    let manifest_bytes = fs::symlink_metadata(&item.manifest)
        .with_context(|| format!("读取基础备份 manifest 失败：{}", item.manifest.display()))?
        .len();
    let manifest_sha256 = sha256_file(&item.manifest).await?;
    let directory = format!("{base_prefix}/{}", item.backup_id);
    let archive = store
        .upload_verified(
            &format!("{directory}/{}", file_name(&item.cipher)?),
            files::open_regular(&item.cipher)?,
            cipher_bytes,
            &sha256,
        )
        .await?;
    let manifest = store
        .upload_verified(
            &format!("{directory}/{}", file_name(&item.manifest)?),
            files::open_regular(&item.manifest)?,
            manifest_bytes,
            &manifest_sha256,
        )
        .await?;
    let record = BaseUploadRecord {
        format: BASE_UPLOAD_FORMAT.to_owned(),
        backup_id: item.backup_id.clone(),
        // 取基础备份完成时刻而不是上传时刻：响应丢失后重试必须生成完全相同的字节。
        created_unix_seconds: item.record.finished_unix_seconds,
        archive: BaseObject::of(&archive),
        manifest: BaseObject::of(&manifest),
    };
    let payload = serde_json::to_vec_pretty(&record).context("序列化基础备份上传记录失败")?;
    let completion = store.upload_record(&format!("{directory}/completed.json"), payload).await?;
    Ok(BaseUploaded { backup_id: item.backup_id.clone(), directory, archive, manifest, completion })
}

impl BaseObject {
    fn of(object: &BackupObject) -> Self {
        Self { key: object.key.clone(), bytes: object.bytes, sha256: object.sha256.clone() }
    }
}

/// 写入 WAL 上传证据：密文/manifest 删除后仍保留；临时文件 fsync 后改名。
fn write_wal_upload_receipt(directory: &Path, receipt: &WalUploadReceipt) -> anyhow::Result<()> {
    let path = wal_upload_path(directory, &receipt.segment_name);
    let temporary = directory.join(format!("{}.wal-upload.tmp", receipt.segment_name));
    let bytes = serde_json::to_vec_pretty(receipt).context("序列化 WAL 上传证据失败")?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)
        .with_context(|| format!("创建 WAL 上传证据失败：{}", temporary.display()))?;
    file.write_all(&bytes).context("写入 WAL 上传证据失败")?;
    file.set_permissions(fs::Permissions::from_mode(0o640)).context("设置 WAL 上传证据权限失败")?;
    file.sync_all().context("同步 WAL 上传证据失败")?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("发布 WAL 上传证据失败：{}", path.display()))?;
    fs::File::open(directory)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("同步 WAL 暂存目录失败：{}", directory.display()))
}

/// 写入本机上传证据：临时文件 fsync 后改名，重复写入只覆盖同一条小证据。
fn write_receipt(
    directory: &Path,
    backup_id: &str,
    destination: &str,
    completion_sha256: &str,
) -> anyhow::Result<()> {
    let receipt = BaseReceipt {
        format: BASE_RECEIPT_FORMAT.to_owned(),
        backup_id: backup_id.to_owned(),
        destination: destination.to_owned(),
        uploaded_unix_seconds: u64::try_from(chrono::Utc::now().timestamp())
            .context("系统时间早于 UNIX 纪元")?,
        completion_sha256: completion_sha256.to_owned(),
    };
    let bytes = serde_json::to_vec_pretty(&receipt).context("序列化基础备份上传证据失败")?;
    let path = base_receipt_path(directory, backup_id);
    let temporary = directory.join(format!("{backup_id}{BASE_RECEIPT_SUFFIX}.tmp"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)
        .with_context(|| format!("创建基础备份上传证据失败：{}", temporary.display()))?;
    file.write_all(&bytes).context("写入基础备份上传证据失败")?;
    file.set_permissions(fs::Permissions::from_mode(0o640))
        .context("设置基础备份上传证据权限失败")?;
    file.sync_all().context("同步基础备份上传证据失败")?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("发布基础备份上传证据失败：{}", path.display()))?;
    fs::File::open(directory)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("同步暂存目录失败：{}", directory.display()))
}

/// 按本地保留计划删除旧基础备份；先删上传证据，使中断后最多多一次远端回读。
fn prune(directory: &Path, plan: Retention, report: &mut BaseReport) {
    report.retained = plan.retained;
    for set in plan.removable {
        let receipt = base_receipt_path(directory, &set.backup_id);
        let removed = remove_if_present(&receipt)
            .and_then(|()| remove_if_present(&set.manifest))
            .and_then(|()| remove_if_present(&set.cipher));
        let synced = removed.and_then(|()| fs::File::open(directory)?.sync_all());
        match synced {
            Ok(()) => report.removed.push(set.backup_id),
            Err(error) => report.failed.push(Failed {
                name: set.backup_id,
                reason: format!("删除超出保留份数的基础备份失败：{error}"),
            }),
        }
    }
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU32, Ordering},
    };

    use super::{
        BASE_PREFIX, BASE_UPLOAD_FORMAT, BaseReport, WAL_PREFIX, destination_identity, object_key,
        prune,
    };
    use crate::backup::wal::model::{
        BASE_MANIFEST_FORMAT, BaseManifest, BaseStaged, base_cipher_path, base_manifest_path,
        base_receipt_path, retention_plan,
    };

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "auction-upload-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("创建测试根目录");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("设置测试根目录权限");
            Self { root }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn set(root: &Path, backup_id: &str, cipher: &[u8]) -> BaseStaged {
        let cipher_path = base_cipher_path(root, backup_id);
        let manifest_path = base_manifest_path(root, backup_id);
        fs::write(&cipher_path, cipher).expect("写入密文");
        let record = BaseManifest {
            format: BASE_MANIFEST_FORMAT.to_owned(),
            backup_id: backup_id.to_owned(),
            started_unix_seconds: 1,
            finished_unix_seconds: 2,
            plaintext_bytes: cipher.len() as u64,
            cipher_bytes: cipher.len() as u64,
            cipher_sha256: "0".repeat(64),
        };
        fs::write(&manifest_path, serde_json::to_vec_pretty(&record).expect("序列化 manifest"))
            .expect("写入 manifest");
        BaseStaged {
            backup_id: backup_id.to_owned(),
            cipher: cipher_path,
            manifest: manifest_path,
            record,
            uploaded: true,
            receipt_destination: None,
        }
    }

    #[test]
    fn object_keys_prefix_wal_and_base_separately() {
        assert_eq!(object_key("", WAL_PREFIX), "wal");
        assert_eq!(object_key("auction", WAL_PREFIX), "auction/wal");
        assert_eq!(object_key("auction", BASE_PREFIX), "auction/base");
        assert_eq!(
            format!("{}/{}", object_key("auction", BASE_PREFIX), "base-20261104T031500Z"),
            "auction/base/base-20261104T031500Z"
        );
        assert_eq!(
            format!("{}/{}", object_key("auction", WAL_PREFIX), "000000010000000000000001.zst.age"),
            "auction/wal/000000010000000000000001.zst.age"
        );
    }

    #[test]
    fn completion_record_format_is_stable() {
        assert_eq!(BASE_UPLOAD_FORMAT, "auction-base-upload.v1");
    }

    #[test]
    fn base_destination_binds_endpoint_bucket_and_prefix() {
        let original = destination_identity("https://r2.example/", "bucket", "auction/base");
        assert_eq!(original, "https://r2.example\u{241f}bucket\u{241f}auction/base");
        assert_ne!(
            original,
            destination_identity("https://other.example", "bucket", "auction/base")
        );
        assert_ne!(original, destination_identity("https://r2.example", "other", "auction/base"));
        assert_ne!(original, destination_identity("https://r2.example", "bucket", "other/base"));
    }

    #[test]
    fn prune_removes_old_sets_and_keeps_newest_three() {
        let fixture = Fixture::new();
        let root = fixture.root.as_path();
        let ids = [
            "base-20261014T031500Z",
            "base-20261021T031500Z",
            "base-20261028T031500Z",
            "base-20261104T031500Z",
        ];
        let sets: Vec<_> = ids.iter().map(|id| set(root, id, b"cipher")).collect();
        // 最旧的组不只有密文和 manifest，还要删掉本机上传证据。
        fs::write(base_receipt_path(root, ids[0]), b"receipt").expect("写入上传证据");
        let plan = retention_plan(sets, 3, &[]);
        let mut report = BaseReport::default();
        prune(root, plan, &mut report);
        assert_eq!(report.removed, vec!["base-20261014T031500Z".to_owned()]);
        assert_eq!(report.retained.len(), 3);
        assert_eq!(report.failed.len(), 0);
        assert!(!base_cipher_path(root, ids[0]).exists());
        assert!(!base_manifest_path(root, ids[0]).exists());
        assert!(!base_receipt_path(root, ids[0]).exists());
        for id in &ids[1..] {
            assert!(base_cipher_path(root, id).exists());
            assert!(base_manifest_path(root, id).exists());
        }
    }

    #[test]
    fn failed_upload_is_not_pruned() {
        let fixture = Fixture::new();
        let root = fixture.root.as_path();
        let ids = [
            "base-20261014T031500Z",
            "base-20261021T031500Z",
            "base-20261028T031500Z",
            "base-20261104T031500Z",
        ];
        let sets: Vec<_> = ids.iter().map(|id| set(root, id, b"cipher")).collect();
        let plan = retention_plan(sets, 3, &[ids[0].to_owned()]);
        let mut report = BaseReport::default();
        prune(root, plan, &mut report);
        assert!(report.removed.is_empty());
        assert!(base_cipher_path(root, ids[0]).exists());
    }
}
