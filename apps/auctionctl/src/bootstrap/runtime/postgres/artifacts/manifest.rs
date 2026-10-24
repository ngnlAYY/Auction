//! PostgreSQL 备份 manifest 与文件摘要校验。

use super::super::*;
use super::{filesystem::open_private_artifact, read_private_artifact};

/// 同时返回解析结果和原始字节，留证摘要必须绑定实际解析的同一快照。
pub(crate) fn read_backup_manifest(
    path: &Path,
) -> anyhow::Result<(PostgresBackupManifest, Vec<u8>)> {
    let bytes = read_private_artifact(path, "backup manifest", MAX_BACKUP_MANIFEST_BYTES)?;
    let manifest = serde_json::from_slice(&bytes).context("decode PostgreSQL backup manifest")?;
    Ok((manifest, bytes))
}

pub(crate) fn validate_backup_manifest(
    manifest: &PostgresBackupManifest,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    if manifest.format != POSTGRES_BACKUP_FORMAT
        || manifest.schema_version <= 0
        || manifest.archive_file_name.is_empty()
        || manifest.archive_file_name != manifest.archive_file_name.trim()
        || manifest.archive_file_name.contains('/')
        || manifest.archive_file_name.contains('\\')
        || manifest.archive_file_name.chars().any(char::is_control)
        || manifest.archive_file_name.ends_with(".manifest")
        || !manifest.archive_file_name.ends_with(".dump")
        || !is_sha256_hex(&manifest.database_name_sha256)
        || !is_sha256_hex(&manifest.archive_sha256)
    {
        anyhow::bail!("backup manifest identity or path fields are invalid");
    }
    if manifest.database_name_sha256 != sha256_text(confirmed_database_name) {
        anyhow::bail!("confirmed database name does not match backup manifest");
    }
    Ok(())
}

pub(crate) fn manifest_archive_path(
    manifest_path: &Path,
    manifest: &PostgresBackupManifest,
) -> anyhow::Result<PathBuf> {
    let manifest_filename = manifest_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("backup manifest filename is not valid UTF-8"))?;
    let expected_manifest_filename = format!("{}.manifest", manifest.archive_file_name);
    if manifest_filename != expected_manifest_filename {
        anyhow::bail!("backup manifest filename is not paired with its archive");
    }
    let parent = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("backup manifest parent is missing"))?;
    Ok(parent.join(&manifest.archive_file_name))
}

pub(crate) fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let (mut file, _) = open_private_artifact(path, "artifact")?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes_read = file.read(&mut buffer).context("read artifact for SHA-256")?;
        if bytes_read == 0 {
            break;
        }
        digest.update(&buffer[..bytes_read]);
    }
    Ok(hex_digest(digest.finalize().as_slice()))
}

pub(crate) fn sha256_text(value: &str) -> String {
    sha256_bytes(value.as_bytes())
}

pub(crate) fn sha256_bytes(value: &[u8]) -> String {
    hex_digest(Sha256::digest(value).as_slice())
}

pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

pub(crate) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
