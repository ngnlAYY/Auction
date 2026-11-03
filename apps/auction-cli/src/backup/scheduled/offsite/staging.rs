//! 与本地备份同文件系统的密文暂存；只回收已完成副本的受控文件，不清理待上传明文。

use anyhow::Context;
use auction_config::AppConfig;

use super::super::{
    disk,
    filesystem::{FileIdentity, PrivateDirectory},
    journal::Journal,
    model::{ArchiveRecord, Failure, RunReceipt},
};
use super::{
    encryption,
    model::{Cipher, Pending},
    replica,
    state::{State, missing},
};

pub(in crate::backup::scheduled) const DIRECTORY: &str = "offsite-staging";

pub(in crate::backup::scheduled) fn reservation(archive: &ArchiveRecord) -> anyhow::Result<u64> {
    encrypted_upper_bound(archive.archive_identity.bytes)?
        .checked_add(encrypted_upper_bound(archive.manifest_identity.bytes)?)
        .context("密文空间预算溢出")
}

fn encrypted_upper_bound(bytes: u64) -> anyhow::Result<u64> {
    // age 原生 X25519 二进制流：每 64 KiB 一个 16 字节认证标签，另保守预留 16 KiB 头部。
    bytes
        .checked_add(bytes.div_ceil(65536).checked_mul(16).context("密文标签空间溢出")?)
        .and_then(|bytes| bytes.checked_add(16384))
        .context("密文空间溢出")
}

pub(super) async fn seal(
    state: &State,
    item: &mut Pending,
    output: &PrivateDirectory,
    config: &AppConfig,
    current: &mut RunReceipt,
    journal: &Journal,
) -> anyhow::Result<PrivateDirectory> {
    let staging = output.child(DIRECTORY)?.child(&item.run_id.to_string())?;
    if item.daily.completion.is_some()
        && (item.write_monthly == Some(false) || item.monthly.completion.is_some())
    {
        return Ok(staging);
    }
    let archive_name = format!("{}.age", item.archive.archive_file_name);
    let manifest_name = format!("{}.age", item.archive.manifest_file_name);
    let mut reserve = 0_u64;
    for (cipher, name, plain_bytes) in [
        (&item.archive_cipher, &archive_name, item.archive.archive_identity.bytes),
        (&item.manifest_cipher, &manifest_name, item.archive.manifest_identity.bytes),
    ] {
        if cipher.is_none() {
            match staging.open(name) {
                Ok(_) => {}
                Err(error) if missing(&error) => {
                    reserve = reserve
                        .checked_add(encrypted_upper_bound(plain_bytes)?)
                        .context("待上传空间预算溢出")?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    let snapshot = disk::inspect(output, &config.backup, reserve)?;
    if !snapshot.passed {
        current.disk_after = Some(snapshot);
        return Err(Failure { stage: current.stage, error_code: "disk_low".to_owned() }.into());
    }
    item.archive_cipher = Some(
        encryption::seal(
            &staging,
            &output.path.join(&item.archive.archive_file_name),
            item.archive.archive_identity,
            &archive_name,
            &config.backup.offsite.age_recipient,
            item.archive_cipher.as_ref(),
        )
        .await?,
    );
    replica::checkpoint(state, item, current, journal)?;
    item.manifest_cipher = Some(
        encryption::seal(
            &staging,
            &output.path.join(&item.archive.manifest_file_name),
            item.archive.manifest_identity,
            &manifest_name,
            &config.backup.offsite.age_recipient,
            item.manifest_cipher.as_ref(),
        )
        .await?,
    );
    replica::checkpoint(state, item, current, journal)?;
    Ok(staging)
}

pub(super) fn remove(
    output: &PrivateDirectory,
    staging: &PrivateDirectory,
    item: &Pending,
) -> anyhow::Result<()> {
    for cipher in [&item.archive_cipher, &item.manifest_cipher].into_iter().flatten() {
        remove_cipher(staging, cipher)?;
        let partial = format!("{}.partial", cipher.file_name);
        match staging.open(&partial) {
            Ok(file) => staging.remove(&partial, FileIdentity::of(&file.metadata()?))?,
            Err(error) if missing(&error) => {}
            Err(error) => return Err(error),
        }
    }
    output.child(DIRECTORY)?.remove_child(staging)
}

fn remove_cipher(staging: &PrivateDirectory, cipher: &Cipher) -> anyhow::Result<()> {
    match staging.open(&cipher.file_name) {
        Ok(file) => {
            if FileIdentity::of(&file.metadata()?) != cipher.identity {
                anyhow::bail!("清理密文前身份变化");
            }
            staging.remove(&cipher.file_name, cipher.identity)
        }
        Err(error) if missing(&error) => Ok(()),
        Err(error) => Err(error),
    }
}
