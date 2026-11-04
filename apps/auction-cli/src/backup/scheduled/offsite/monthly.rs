//! 每个目的地、每个 UTC 月只创建第一份完成副本；本机标记丢失时从远端完成记录恢复。

use anyhow::Context;
use auction_integrations::{BackupObject, BackupStoreError, R2BackupStore};

use super::{
    model::{MonthMarker, Pending, UPLOAD_FORMAT, UploadRecord},
    state::{State, missing},
};
use crate::{app::progress, backup::manifest::sha256_bytes};

const MARKER_FORMAT: &str = "auction-backup-month.v1";

pub(super) async fn exists(
    store: &R2BackupStore,
    state: &State,
    item: &Pending,
    prefix: &str,
) -> anyhow::Result<bool> {
    let month = item.archive.created_at.format("%Y-%m").to_string();
    let name = marker_name(item);
    let root =
        super::object_key(prefix, &format!("monthly/{}", item.archive.created_at.format("%Y/%m")));
    match state.months.read(&name) {
        Ok(bytes) => {
            let marker: MonthMarker = serde_json::from_slice(&bytes).context("月副本标记无效")?;
            if marker.format != MARKER_FORMAT
                || marker.destination_sha256 != item.destination_sha256
                || marker.month != month
                || !marker.completion.key.starts_with(&format!("{root}/"))
                || !marker.completion.key.ends_with("/completed.json")
            {
                anyhow::bail!("月副本标记身份不一致");
            }
            return Ok(true);
        }
        Err(error) if missing(&error) => {}
        Err(error) => return Err(error),
    }
    for key in store.completion_records(&root).await? {
        match inspect_record(store, &key, &root, &month).await {
            Ok(completion) => {
                save(state, item, completion)?;
                return Ok(true);
            }
            Err(
                BackupStoreError::RemoteRecord
                | BackupStoreError::Conflict
                | BackupStoreError::Configuration,
            ) => {
                // 单个坏恢复点不能永久阻断整月；不打印记录正文、对象名或底层错误。
                progress(format_args!("warning=monthly_record_invalid"));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

async fn inspect_record(
    store: &R2BackupStore,
    key: &str,
    root: &str,
    month: &str,
) -> Result<BackupObject, BackupStoreError> {
    let bytes = store.read_record(key).await?;
    let record: UploadRecord =
        serde_json::from_slice(&bytes).map_err(|_| BackupStoreError::RemoteRecord)?;
    validate_record(&record, key, root, month).map_err(|_| BackupStoreError::RemoteRecord)?;
    // LIST 结果不是完成证据：记录及两份密文均需核对长度和覆盖全部区间的完整摘要。
    let completion = store
        .verify_existing(key, bytes.len() as u64, &sha256_bytes(&bytes))
        .await?
        .ok_or(BackupStoreError::RemoteRecord)?;
    for object in [&record.archive, &record.manifest] {
        store
            .verify_existing(&object.key, object.bytes, &object.sha256)
            .await?
            .ok_or(BackupStoreError::RemoteRecord)?;
    }
    Ok(completion)
}

/// 新任务先完成月副本后，旧任务不能凭先前的未完成选择再制造第二个有效月恢复点。
pub(super) async fn should_write(
    store: &R2BackupStore,
    state: &State,
    item: &Pending,
    prefix: &str,
) -> anyhow::Result<bool> {
    if !exists(store, state, item, prefix).await? {
        return Ok(true);
    }
    let marker: MonthMarker = serde_json::from_slice(&state.months.read(&marker_name(item))?)
        .context("月副本标记无效")?;
    let own_key = super::object_key(
        prefix,
        &format!(
            "monthly/{}/{}/completed.json",
            item.archive.created_at.format("%Y/%m"),
            item.run_id
        ),
    );
    Ok(marker.completion.key == own_key)
}

pub(super) fn save(state: &State, item: &Pending, completion: BackupObject) -> anyhow::Result<()> {
    let marker = MonthMarker {
        format: MARKER_FORMAT.to_owned(),
        destination_sha256: item.destination_sha256.clone(),
        month: item.archive.created_at.format("%Y-%m").to_string(),
        completion,
    };
    let bytes = serde_json::to_vec_pretty(&marker)?;
    let name = marker_name(item);
    // 同一状态锁下的缓存无覆盖发布；若上次已写入，保留原第一份完成事实。
    match state.months.read(&name) {
        Ok(_) => Ok(()),
        Err(error) if missing(&error) => state.months.write(&name, &bytes, false),
        Err(error) => Err(error),
    }
}

fn marker_name(item: &Pending) -> String {
    format!("{}-{}.json", item.destination_sha256, item.archive.created_at.format("%Y-%m"))
}

fn validate_record(
    record: &UploadRecord,
    key: &str,
    root: &str,
    month: &str,
) -> anyhow::Result<()> {
    let base = format!("{root}/{}", record.run_id);
    let archive =
        record.archive.key.strip_prefix(&format!("{base}/")).context("月副本 archive 越界")?;
    let plain_name = archive.strip_suffix(".age").context("月副本 archive 名称无效")?;
    let digest = |value: &str| {
        value.len() == 64
            && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if record.format != UPLOAD_FORMAT
        || record.run_id.is_nil()
        || key != format!("{base}/completed.json")
        || record.created_at.format("%Y-%m").to_string() != month
        || !plain_name.starts_with("scheduled-")
        || !plain_name.ends_with(&format!("-{}.dump", record.run_id))
        || archive.contains('/')
        || record.manifest.key != format!("{base}/{plain_name}.manifest.age")
        || !digest(&record.plaintext_manifest_sha256)
        || !digest(&record.archive.sha256)
        || !digest(&record.manifest.sha256)
        || record.archive.bytes == 0
        || record.manifest.bytes == 0
        || record.age_version.is_empty()
        || record.age_version.len() > 256
        || record.age_version.chars().any(char::is_control)
    {
        anyhow::bail!("月副本完成记录字段无效");
    }
    Ok(())
}
