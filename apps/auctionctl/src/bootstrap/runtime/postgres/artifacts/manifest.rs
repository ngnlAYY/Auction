//! PostgreSQL 备份 manifest 与文件摘要校验。

use super::super::*;
use super::read_private_artifact;

pub(crate) fn read_backup_manifest(path: &Path) -> anyhow::Result<PostgresBackupManifest> {
    let bytes = read_private_artifact(path, "backup manifest", MAX_BACKUP_MANIFEST_BYTES)?;
    serde_json::from_slice(&bytes).context("decode PostgreSQL backup manifest")
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
    let mut file =
        fs::File::open(path).with_context(|| format!("open artifact: {}", path.display()))?;
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
    let mut digest = Sha256::new();
    digest.update(value.as_bytes());
    hex_digest(digest.finalize().as_slice())
}

pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
