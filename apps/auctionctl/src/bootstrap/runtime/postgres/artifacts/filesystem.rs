//! PostgreSQL 备份与证据文件的路径、权限和原子发布约束。

use super::super::*;

pub(crate) fn require_dump_extension(path: &Path) -> anyhow::Result<()> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("dump") {
        anyhow::bail!("PostgreSQL backup output must use the .dump extension");
    }
    Ok(())
}

pub(crate) fn backup_manifest_path(archive_path: &Path) -> anyhow::Result<PathBuf> {
    let filename = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("backup archive filename is not valid UTF-8"))?;
    Ok(archive_path.with_file_name(format!("{filename}.manifest")))
}

pub(crate) fn require_absent_artifact(path: &Path, label: &str) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("{label} already exists; refusing to overwrite"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {label}: {}", path.display())),
    }
}

pub(crate) fn create_private_temp_path(target: &Path, label: &str) -> anyhow::Result<PathBuf> {
    let parent =
        target.parent().ok_or_else(|| anyhow::anyhow!("private artifact parent is missing"))?;
    for _ in 0..8 {
        let candidate = parent.join(format!(".{label}-{}.tmp", Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("create private temporary artifact: {}", candidate.display())
                });
            }
        }
    }
    anyhow::bail!("could not allocate a private temporary backup artifact")
}

pub(crate) fn publish_private_artifact(temporary: &Path, target: &Path) -> anyhow::Result<()> {
    require_absent_artifact(target, "backup archive")?;
    fs::hard_link(temporary, target)
        .with_context(|| format!("publish backup archive: {}", target.display()))?;
    fs::remove_file(temporary)
        .with_context(|| format!("remove backup temporary artifact: {}", temporary.display()))?;
    Ok(())
}

pub(crate) fn remove_owned_temp(path: &Path) {
    let _ = fs::remove_file(path);
}

pub(crate) fn verify_private_regular_file(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("read {label} metadata: {}", path.display()))?;
    if !metadata.file_type().is_file() {
        anyhow::bail!("{label} is not a regular non-symlink file: {}", path.display());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("{label} allows group/other access: {}", path.display());
    }
    if metadata.len() == 0 {
        anyhow::bail!("{label} is empty: {}", path.display());
    }
    Ok(())
}

pub(crate) fn validate_private_evidence_path(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() || path == Path::new("/") {
        anyhow::bail!("private evidence output must be a non-root absolute path");
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("private evidence output parent is missing"))?;
    let canonical_parent = fs::canonicalize(parent)
        .with_context(|| format!("canonicalize output parent: {}", parent.display()))?;
    if canonical_parent != parent {
        anyhow::bail!("private evidence output parent must be a canonical non-symlink path");
    }
    if path.file_name().is_none() {
        anyhow::bail!("private evidence output filename is missing");
    }
    Ok(())
}

pub(crate) fn write_private_evidence(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    validate_private_evidence_path(path)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("retention inventory output parent is missing"))?;
    let metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("read retention inventory parent: {}", parent.display()))?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!("retention inventory output parent is not a directory: {}", parent.display());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!(
            "retention inventory output parent is group/other writable: {}",
            parent.display()
        );
    }

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("create retention inventory evidence: {}", path.display()))?;
    file.write_all(contents).context("write retention inventory evidence")?;
    file.write_all(b"\n").context("terminate retention inventory evidence")?;
    file.sync_all().context("sync retention inventory evidence")?;
    Ok(())
}
