//! 目标存储、单段与基础备份上传、回执写入与本地清理。

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::Context;
use auction_config::AppConfig;
use auction_integrations::{BackupObject, R2BackupStore};

use super::{BASE_UPLOAD_FORMAT, BaseObject, BaseReport, BaseUploadRecord, BaseUploaded, Failed};
use crate::backup::manifest::sha256_file;
use crate::backup::wal::model::{
    BASE_RECEIPT_FORMAT, BASE_RECEIPT_SUFFIX, BaseReceipt, BaseStaged, Retention, Staged,
    WalUploadReceipt, base_receipt_path, wal_upload_path,
};
use crate::support::files;

/// 基础备份上传证据的目标身份；只含 endpoint、bucket 与前缀，不含任何凭据。
pub(super) fn base_destination(config: &AppConfig, base_prefix: &str) -> String {
    destination_identity(
        &config.backup.offsite.endpoint,
        &config.backup.offsite.bucket,
        base_prefix,
    )
}

pub(super) fn destination_identity(endpoint: &str, bucket: &str, base_prefix: &str) -> String {
    // 使用非控制字符分隔，避免证据校验把合法身份误判为损坏。
    format!("{}\u{241f}{bucket}\u{241f}{base_prefix}", endpoint.trim_end_matches('/'))
}

pub(super) fn store(config: &AppConfig) -> anyhow::Result<R2BackupStore> {
    let offsite = &config.backup.offsite;
    Ok(R2BackupStore::new(
        &offsite.endpoint,
        &offsite.bucket,
        config.backup_r2_access_key_id().context("缺少 R2 access key")?,
        config.backup_r2_secret_access_key().context("缺少 R2 secret")?,
    )?)
}

/// 上传单个暂存段：本地长度与 manifest 一致、摘要由本机计算，远端只做长度与完整回读校验。
pub(super) async fn upload_one(
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
pub(super) async fn upload_base(
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

/// 写入 WAL 上传证据：密文/manifest 删除后仍保留；临时文件 fsync 后改名。
pub(super) fn write_wal_upload_receipt(
    directory: &Path,
    receipt: &WalUploadReceipt,
) -> anyhow::Result<()> {
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
    // 上传器以 auction 运行、PostgreSQL archive_command 以 postgres 运行；证据不含秘密，
    // 需要让 archive 账号能读，否则重试时 classify 会因权限失败而阻塞 pg_wal。
    file.set_permissions(fs::Permissions::from_mode(0o644)).context("设置 WAL 上传证据权限失败")?;
    file.sync_all().context("同步 WAL 上传证据失败")?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("发布 WAL 上传证据失败：{}", path.display()))?;
    fs::File::open(directory)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("同步 WAL 暂存目录失败：{}", directory.display()))
}

/// 写入本机上传证据：临时文件 fsync 后改名，重复写入只覆盖同一条小证据。
pub(super) fn write_receipt(
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
pub(super) fn prune(directory: &Path, plan: Retention, report: &mut BaseReport) {
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

pub(super) fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn remove_local(item: &Staged) -> anyhow::Result<()> {
    fs::remove_file(&item.cipher)
        .with_context(|| format!("删除已上传的 WAL 密文失败：{}", item.cipher.display()))?;
    fs::remove_file(&item.manifest)
        .with_context(|| format!("删除已上传的 WAL manifest 失败：{}", item.manifest.display()))?;
    let directory = item.cipher.parent().context("WAL 密文缺少父目录")?;
    fs::File::open(directory)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("同步 WAL 暂存目录失败：{}", directory.display()))
}

pub(super) fn file_name(path: &Path) -> anyhow::Result<&str> {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("WAL 密文文件名无效：{}", path.display()))
}

pub(super) fn object_key(prefix: &str, tail: &str) -> String {
    if prefix.is_empty() { tail.to_owned() } else { format!("{prefix}/{tail}") }
}
