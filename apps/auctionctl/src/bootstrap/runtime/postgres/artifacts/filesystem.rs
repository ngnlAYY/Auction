//! PostgreSQL 备份与证据文件的路径、权限和原子发布约束。

use super::super::*;
use std::marker::PhantomData;

mod parents;

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
    validate_private_evidence_path(target)?;
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
    sync_private_artifact(temporary)?;
    require_absent_artifact(target, "backup archive")?;
    fs::hard_link(temporary, target)
        .with_context(|| format!("publish backup archive: {}", target.display()))?;
    fs::remove_file(temporary)
        .with_context(|| format!("remove backup temporary artifact: {}", temporary.display()))?;
    sync_artifact_parent(target)?;
    if temporary.parent() != target.parent() {
        sync_artifact_parent(temporary)?;
    }
    Ok(())
}

pub(crate) fn remove_owned_temp(path: &Path) {
    let _ = fs::remove_file(path);
}

pub(crate) fn verify_private_regular_file(path: &Path, label: &str) -> anyhow::Result<()> {
    validate_private_evidence_path(path)?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("read {label} metadata: {}", path.display()))?;
    if !metadata.file_type().is_file() {
        anyhow::bail!("{label} is not a regular non-symlink file: {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        // 文件由当前有效 UID 拥有；目录链同时排除其他 UID 替换目录项的能力。
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            anyhow::bail!(
                "{label} must be owner-owned and deny group/other access: {}",
                path.display()
            );
        }
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
    parents::verify_parent_chain(parent)?;
    if path.file_name().is_none() {
        anyhow::bail!("private evidence output filename is missing");
    }
    require_directory_sync_support()
}

/// 统一校验私有证据回执的目标路径、父目录权限并预留不可覆盖的临时文件。
fn reserve_private_artifact(
    target: &Path,
    artifact_label: &str,
    parent_label: &str,
    temporary_label: &str,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    validate_private_evidence_path(target)?;
    require_absent_artifact(target, artifact_label)?;
    let parent =
        target.parent().ok_or_else(|| anyhow::anyhow!("{parent_label} parent is missing"))?;
    let metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("read {parent_label} parent: {}", parent.display()))?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!("{parent_label} parent is not a directory: {}", parent.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o022 != 0 {
            anyhow::bail!("{parent_label} parent is group/other writable: {}", parent.display());
        }
    }
    // 在数据库 mutation 前先验证目标文件系统支持目录同步。
    sync_artifact_parent(target)?;
    let temporary_path = create_private_temp_path(target, temporary_label)?;
    Ok((target.to_owned(), temporary_path))
}

/// 需要发布回执的 PostgreSQL 运维命令类型；标签集中定义以避免安全错误信息漂移。
#[derive(Clone, Copy)]
pub(crate) enum PrivateReceiptKind {
    Rebuild,
    LegacyV28Adoption,
    Restore,
}

struct PrivateReceiptLabels {
    artifact_label: &'static str,
    parent_label: &'static str,
    temporary_label: &'static str,
    open_context: &'static str,
    write_context: &'static str,
    terminate_context: &'static str,
    sync_context: &'static str,
    publish_context: &'static str,
    cleanup_context: &'static str,
}

impl PrivateReceiptKind {
    const fn labels(self) -> PrivateReceiptLabels {
        match self {
            Self::Rebuild => PrivateReceiptLabels {
                artifact_label: "rebuild receipt",
                parent_label: "rebuild receipt",
                temporary_label: "auction-postgres-rebuild-receipt",
                open_context: "open rebuild receipt temporary file",
                write_context: "write PostgreSQL rebuild receipt",
                terminate_context: "terminate PostgreSQL rebuild receipt",
                sync_context: "sync PostgreSQL rebuild receipt",
                publish_context: "publish PostgreSQL rebuild receipt",
                cleanup_context: "rebuild receipt published but temporary cleanup failed",
            },
            Self::LegacyV28Adoption => PrivateReceiptLabels {
                artifact_label: "legacy v28 adoption receipt",
                parent_label: "adoption receipt",
                temporary_label: "auction-postgres-legacy-v28-receipt",
                open_context: "open adoption receipt temporary file",
                write_context: "write legacy v28 adoption receipt",
                terminate_context: "terminate legacy v28 adoption receipt",
                sync_context: "sync legacy v28 adoption receipt",
                publish_context: "publish legacy v28 adoption receipt",
                cleanup_context: "adoption committed and receipt published, but temporary receipt cleanup failed",
            },
            Self::Restore => PrivateReceiptLabels {
                artifact_label: "restore receipt",
                parent_label: "restore receipt",
                temporary_label: "auction-postgres-restore-receipt",
                open_context: "open restore receipt temporary file",
                write_context: "write PostgreSQL restore receipt",
                terminate_context: "terminate PostgreSQL restore receipt",
                sync_context: "sync PostgreSQL restore receipt",
                publish_context: "publish PostgreSQL restore receipt",
                cleanup_context: "restore committed and receipt published, but temporary receipt cleanup failed",
            },
        }
    }
}

pub(crate) trait PrivateReceiptSpec {
    const KIND: PrivateReceiptKind;
}

pub(crate) struct RebuildReceiptSpec;

impl PrivateReceiptSpec for RebuildReceiptSpec {
    const KIND: PrivateReceiptKind = PrivateReceiptKind::Rebuild;
}

pub(crate) struct LegacyV28AdoptionReceiptSpec;

impl PrivateReceiptSpec for LegacyV28AdoptionReceiptSpec {
    const KIND: PrivateReceiptKind = PrivateReceiptKind::LegacyV28Adoption;
}

pub(crate) struct RestoreReceiptSpec;

impl PrivateReceiptSpec for RestoreReceiptSpec {
    const KIND: PrivateReceiptKind = PrivateReceiptKind::Restore;
}

/// 共享三类 PostgreSQL 回执的预留、一次性发布和失败清理状态。
pub(crate) struct PrivateReceiptReservation<S: PrivateReceiptSpec> {
    target_path: PathBuf,
    temporary_path: PathBuf,
    published: bool,
    _spec: PhantomData<S>,
}

impl<S: PrivateReceiptSpec> PrivateReceiptReservation<S> {
    pub(crate) fn reserve(target_path: &Path) -> anyhow::Result<Self> {
        let labels = S::KIND.labels();
        let (target_path, temporary_path) = reserve_private_artifact(
            target_path,
            labels.artifact_label,
            labels.parent_label,
            labels.temporary_label,
        )?;
        Ok(Self { target_path, temporary_path, published: false, _spec: PhantomData })
    }

    pub(crate) fn publish(mut self, contents: &[u8]) -> anyhow::Result<()> {
        publish_private_receipt(&self.temporary_path, &self.target_path, contents, S::KIND)?;
        self.published = true;
        Ok(())
    }
}

impl<S: PrivateReceiptSpec> Drop for PrivateReceiptReservation<S> {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}

fn publish_private_receipt(
    temporary: &Path,
    target: &Path,
    contents: &[u8],
    kind: PrivateReceiptKind,
) -> anyhow::Result<()> {
    let labels = kind.labels();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(temporary)
        .with_context(|| format!("{}: {}", labels.open_context, temporary.display()))?;
    file.write_all(contents).context(labels.write_context)?;
    file.write_all(b"\n").context(labels.terminate_context)?;
    file.sync_all().context(labels.sync_context)?;
    drop(file);

    require_absent_artifact(target, labels.artifact_label)?;
    fs::hard_link(temporary, target)
        .with_context(|| format!("{}: {}", labels.publish_context, target.display()))?;
    fs::remove_file(temporary)
        .with_context(|| format!("{}: {}", labels.cleanup_context, temporary.display()))?;
    sync_artifact_parent(target)?;
    if temporary.parent() != target.parent() {
        sync_artifact_parent(temporary)?;
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
    sync_artifact_parent(path)
}

/// 在破坏性数据库操作前证明恢复文件内容及其目录项已经完成持久化。
pub(crate) fn sync_private_artifact(path: &Path) -> anyhow::Result<()> {
    verify_private_regular_file(path, "recovery artifact")?;
    fs::File::open(path)
        .context("打开灾备文件以同步失败")?
        .sync_all()
        .context("同步灾备文件内容失败")?;
    sync_artifact_parent(path)
}

fn sync_artifact_parent(path: &Path) -> anyhow::Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow::anyhow!("灾备文件父目录缺失"))?;
    #[cfg(unix)]
    {
        // 文件 fsync 不保证 hard_link/create/remove 的目录项持久；成功发布必须同步目录。
        fs::File::open(parent)
            .context("打开灾备父目录以同步失败")?
            .sync_all()
            .context("同步灾备父目录失败")
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        anyhow::bail!("当前平台不支持灾备目录持久化证明，拒绝发布或使用灾备文件")
    }
}

fn require_directory_sync_support() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        Ok(())
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!("当前平台不支持灾备目录持久化证明")
    }
}
