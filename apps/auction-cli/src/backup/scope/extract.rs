//! 范围证据的离线解密与恢复校验：先完整验证密文与打包摘要，再逐个写入全新私有目录。

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use anyhow::Context;
use chrono::Utc;

use super::{PlaintextGuard, bundle, collect::SourceKind, crypto};
use crate::{
    app::{print_json, progress},
    backup::manifest::sha256_file,
    support::files,
};

const RECEIPT_FORMAT: &str = "auction-backup-scope-extract.v1";

pub(crate) struct Options<'a> {
    pub(crate) manifest: &'a Path,
    pub(crate) database: &'a str,
    pub(crate) identity: &'a Path,
    pub(crate) output_dir: &'a Path,
    pub(crate) receipt: Option<&'a Path>,
}

pub(crate) async fn run(options: Options<'_>) -> anyhow::Result<()> {
    if let Some(receipt) = options.receipt {
        files::require_private_target(receipt)?;
    }
    let verified = crate::backup::verify_pair(options.manifest, options.database).await?;
    let scope =
        verified.scope.as_ref().context("该恢复点没有备份范围 sidecar；不能凭空生成范围证据")?;
    prepare_output(options.output_dir)?;
    let plaintext = options.output_dir.join(".scope.plaintext.partial");
    let mut guard = PlaintextGuard::new(plaintext.clone());
    crypto::decrypt_file(options.identity, &scope.bundle_path, &plaintext).await?;
    let actual = sha256_file(&plaintext).await?;
    if actual != scope.sidecar.plaintext_sha256 {
        anyhow::bail!("解密后的范围打包文件摘要与 sidecar 不一致");
    }
    progress("已验证范围密文；开始逐文件校验");
    // 第一遍只验证打包结构与摘要，避免把不完整的恢复材料写进输出目录。
    let checked =
        bundle::read(open(&plaintext)?, |_, _, _| Ok(())).context("校验范围打包文件失败")?;
    if checked.bytes != scope.sidecar.plaintext_bytes
        || checked.sha256 != scope.sidecar.plaintext_sha256
        || checked.entries.len() as u64 != scope.sidecar.files_count
    {
        anyhow::bail!("范围打包文件的规模与 sidecar 不一致");
    }
    for entry in &scope.sidecar.files {
        let Some(found) = checked.entries.iter().find(|candidate| candidate.path == entry.path)
        else {
            anyhow::bail!("sidecar 列出的文件不在打包文件中：{}", entry.path);
        };
        if found.kind != entry.kind || found.bytes != entry.bytes || found.sha256 != entry.sha256 {
            anyhow::bail!("sidecar 与打包文件的条目摘要不一致：{}", entry.path);
        }
    }
    // 第二遍写入：输出目录在开始时为空，每个文件都以 create_new/0600 新建。
    let mut writer = Extractor::new(options.output_dir.to_owned());
    bundle::read(open(&plaintext)?, |kind, path, chunk| writer.write(kind, path, chunk))?;
    let written = writer.finish()?;
    guard.remove()?;
    let payload = serde_json::json!({
        "format": RECEIPT_FORMAT,
        "extracted_at": Utc::now(),
        "database": options.database,
        "archive_file_name": verified.manifest.archive_file_name,
        "archive_sha256": verified.manifest.archive_sha256,
        "sidecar_sha256": scope.sidecar_sha256,
        "bundle_sha256": scope.sidecar.bundle_sha256,
        "plaintext_sha256": scope.sidecar.plaintext_sha256,
        "files_count": written,
        "sources": scope.sidecar.sources.iter().map(|source| serde_json::json!({
            "kind": source.kind.slug(),
            "files": source.files,
            "bytes": source.bytes,
        })).collect::<Vec<_>>(),
        "output_dir": options.output_dir,
    });
    if let Some(receipt) = options.receipt {
        files::write_private(receipt, &serde_json::to_vec_pretty(&payload)?)
            .context("范围已解出，但回执写入失败")?;
    }
    print_json(&serde_json::json!({
        "outcome": "extracted",
        "output_dir": options.output_dir,
        "files_count": written,
        "sources": payload["sources"],
        "receipt": options.receipt,
        "next_step": "人工核对 runtime 配置与部署回执；恢复后不要覆盖现有配置目录",
    }))
}

fn prepare_output(output_dir: &Path) -> anyhow::Result<()> {
    super::collect::validate_absolute(output_dir)?;
    match fs::symlink_metadata(output_dir) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                anyhow::bail!("范围输出必须是真实目录：{}", output_dir.display());
            }
            let mut entries = fs::read_dir(output_dir).context("枚举范围输出目录失败")?;
            if entries.next().is_some() {
                anyhow::bail!("范围输出目录必须为空，拒绝覆盖已有文件");
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("读取范围输出目录失败"),
    }
    files::ensure_private_dir(output_dir)
}

fn open(path: &Path) -> anyhow::Result<File> {
    crypto::open_plain(path)
}

/// 逐文件写入器：路径来自已校验的相对路径，父目录按需创建，文件拒绝覆盖。
struct Extractor {
    root: PathBuf,
    current: Option<(String, File)>,
    files: u64,
}

impl Extractor {
    fn new(root: PathBuf) -> Self {
        Self { root, current: None, files: 0 }
    }

    fn write(&mut self, _kind: &SourceKind, path: &str, chunk: &[u8]) -> anyhow::Result<()> {
        if self.current.as_ref().is_none_or(|(current, _)| current != path) {
            if let Some((previous, file)) = self.current.take() {
                file.sync_all().with_context(|| format!("同步范围输出文件失败：{previous}"))?;
                files::sync_parent(&self.root.join(&previous))?;
            }
            let target = self.root.join(path);
            if !target.starts_with(&self.root) {
                anyhow::bail!("范围条目路径越界");
            }
            if let Some(parent) = target.parent() {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .context("创建范围输出子目录失败")?;
            }
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&target)
                .with_context(|| format!("创建范围输出文件失败：{}", target.display()))?;
            self.current = Some((path.to_owned(), file));
            self.files = self.files.saturating_add(1);
        }
        if chunk.is_empty() {
            return Ok(());
        }
        let (_, file) = self.current.as_mut().context("范围条目缺少已打开文件")?;
        file.write_all(chunk).context("写入范围输出文件失败")
    }

    fn finish(mut self) -> anyhow::Result<u64> {
        if let Some((path, file)) = self.current.take() {
            file.sync_all().with_context(|| format!("同步范围输出文件失败：{path}"))?;
            let parent = self.root.join(&path);
            files::sync_parent(&parent)?;
        }
        Ok(self.files)
    }
}
