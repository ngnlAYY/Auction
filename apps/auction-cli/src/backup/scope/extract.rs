//! 范围证据的离线解密与恢复校验：先完整验证密文与打包摘要，再逐个写入全新私有目录。

use std::{
    collections::BTreeSet,
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::Context;
use auction_time::Timestamp;
use tokio::process::Command;

use super::{bundle, collect::SourceKind};
use crate::{
    app::{print_json, progress},
    support::{digest::sha256_reader, files},
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
    // 回执与 JSON 输出按 UTF-8 路径投影；提前拒绝无法表示的路径，避免解出证据后 panic。
    for path in [Some(options.output_dir), options.receipt].into_iter().flatten() {
        anyhow::ensure!(
            path.to_str().is_some(),
            "路径必须能被 JSON 回执表示为 UTF-8：{}",
            path.display()
        );
    }
    let verified = crate::backup::verify_pair(options.manifest, options.database).await?;
    let scope =
        verified.scope.as_ref().context("该恢复点没有备份范围 sidecar；不能凭空生成范围证据")?;
    let output_dir = prepare_output(options.output_dir)?;
    let mut plaintext = StagingFile::create(&output_dir)?;
    decrypt_to_file(options.identity, &scope.bundle_path, &plaintext.file).await?;
    let (actual, plaintext_bytes) =
        sha256_reader(plaintext.open_read()?).context("读取范围明文暂存失败")?;
    if actual != scope.sidecar.plaintext_sha256 {
        anyhow::bail!("解密后的范围打包文件摘要与 sidecar 不一致");
    }
    progress("已验证范围密文；开始逐文件校验");
    // 第一遍只验证打包结构与摘要，避免把不完整的恢复材料写进输出目录。
    let checked =
        bundle::read(plaintext.open_read()?, |_, _, _| Ok(())).context("校验范围打包文件失败")?;
    if plaintext_bytes != scope.sidecar.plaintext_bytes
        || checked.bytes != scope.sidecar.plaintext_bytes
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
    bundle::read(plaintext.open_read()?, |kind, path, chunk| writer.write(kind, path, chunk))?;
    let written = writer.finish()?;
    plaintext.remove()?;
    let payload = serde_json::json!({
        "format": RECEIPT_FORMAT,
        "extracted_at": Timestamp::now(),
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

fn prepare_output(output_dir: &Path) -> anyhow::Result<File> {
    super::collect::validate_absolute(output_dir)?;
    files::ensure_private_dir(output_dir)?;
    // 与来源目录共用祖先信任口径：逐级打开并复核，只有当前账号或 root 拥有的祖先（root 的
    // sticky 目录除外）才不会被他人改名或替换，随后的路径式写入因此仍落在受控目录树里。
    // 叶子目录的类型、符号链接与权限也在这里一并复核。
    let directory = super::collect::open_directory(output_dir).context("固定范围输出目录失败")?;
    let metadata = directory.metadata().context("读取范围输出目录元数据失败")?;
    if !metadata.is_dir()
        || metadata.uid() != files::effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("范围输出目录不符合当前账号私有目录策略");
    }
    if fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))
        .context("枚举固定范围输出目录失败")?
        .next()
        .is_some()
    {
        anyhow::bail!("范围输出目录必须为空，拒绝覆盖已有文件");
    }
    Ok(directory)
}

struct StagingFile {
    directory: File,
    name: CString,
    file: File,
    identity: (u64, u64),
}

impl StagingFile {
    fn create(directory: &File) -> anyhow::Result<Self> {
        let name = format!(".scope.plaintext.{}.partial", uuid::Uuid::new_v4());
        let name = CString::new(name).context("范围明文暂存名含 NUL")?;
        // SAFETY: pinned output directory and a generated single path component.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error()).context("创建范围明文暂存失败");
        }
        // SAFETY: openat returned a fresh descriptor owned by this call.
        let file = unsafe { File::from_raw_fd(descriptor) };
        let metadata = file.metadata().context("读取范围明文暂存元数据失败")?;
        if !metadata.is_file()
            || metadata.uid() != files::effective_uid()
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            anyhow::bail!("范围明文暂存文件不符合私有文件策略");
        }
        Ok(Self {
            directory: directory.try_clone().context("复制范围输出目录句柄失败")?,
            name,
            identity: (metadata.dev(), metadata.ino()),
            file,
        })
    }

    fn open_read(&self) -> anyhow::Result<File> {
        // SAFETY: pinned parent and this invocation's component name.
        let descriptor = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error()).context("打开范围明文暂存失败");
        }
        // SAFETY: openat returned a fresh descriptor owned by this call.
        let file = unsafe { File::from_raw_fd(descriptor) };
        let metadata = file.metadata().context("读取范围明文暂存元数据失败")?;
        if !metadata.is_file()
            || metadata.uid() != files::effective_uid()
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
            || (metadata.dev(), metadata.ino()) != self.identity
        {
            anyhow::bail!("范围明文暂存文件身份或权限发生变化");
        }
        Ok(file)
    }

    fn remove(&mut self) -> anyhow::Result<()> {
        let current = self.open_read()?;
        drop(current);
        // SAFETY: only unlink this invocation's name within its pinned directory.
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error()).context("删除范围明文暂存失败");
        }
        self.directory.sync_all().context("同步范围输出目录失败")
    }
}

impl Drop for StagingFile {
    fn drop(&mut self) {
        if self.open_read().is_ok() {
            // SAFETY: cleanup is scoped to this invocation's verified inode and pinned directory.
            unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) };
        }
    }
}

pub(super) async fn decrypt_to_file(
    identity: &Path,
    input: &Path,
    output: &File,
) -> anyhow::Result<()> {
    if !identity.is_absolute() {
        anyhow::bail!("age 私钥必须使用绝对路径");
    }
    let identity_file = files::open_regular(identity)?;
    let metadata = identity_file.metadata().context("读取 age 私钥元数据失败")?;
    if metadata.uid() != files::effective_uid()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!("age 私钥必须是当前账号拥有的 owner-only 单链接普通文件");
    }
    let mut child = Command::new("age")
        .args(["--decrypt", "--identity"])
        .arg(identity)
        .arg("--output")
        .arg("-")
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone().context("复制范围明文句柄失败")?))
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("age 解密无法启动")?;
    let status = child.wait().await.context("等待 age 解密失败")?;
    if !status.success() {
        anyhow::bail!("age 解密失败；请核对离线私钥与备份收件人");
    }
    output.sync_all().context("同步范围明文暂存失败")?;
    Ok(())
}

/// 逐文件写入器：路径来自已校验的相对路径，父目录按需创建，文件拒绝覆盖。
struct Extractor {
    root: PathBuf,
    current: Option<(String, File)>,
    files: u64,
    /// 本次写入用到的相对父目录；`finish` 逐级补同步，崩溃后新建目录项仍可见。
    directories: BTreeSet<PathBuf>,
}

impl Extractor {
    fn new(root: PathBuf) -> Self {
        Self { root, current: None, files: 0, directories: BTreeSet::new() }
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
            if let Some(relative_parent) = Path::new(path).parent()
                && !relative_parent.as_os_str().is_empty()
            {
                self.directories.insert(relative_parent.to_owned());
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
            files::sync_parent(&self.root.join(&path))?;
        }
        // 新建的每一层目录都要在其父目录同步后才在崩溃后保留；从最深一层补齐到输出根。
        let mut synced = BTreeSet::new();
        for relative in std::mem::take(&mut self.directories) {
            let mut level = self.root.join(relative);
            while synced.insert(level.clone()) {
                files::sync_directory(&level, "范围输出目录")?;
                if level == self.root || !level.pop() {
                    break;
                }
            }
        }
        Ok(self.files)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    };

    use super::StagingFile;

    #[test]
    fn staging_files_are_unique_and_cleanup_is_invocation_scoped() {
        let directory_path =
            std::env::temp_dir().join(format!("auction-scope-staging-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&directory_path).expect("建立测试目录");
        let directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&directory_path)
            .expect("固定测试目录");
        let mut first = StagingFile::create(&directory).expect("首个暂存文件");
        let mut second = StagingFile::create(&directory).expect("第二个暂存文件");
        assert_ne!(first.name.as_bytes(), second.name.as_bytes());
        first.file.write_all(b"first invocation").expect("写入首个暂存文件");
        second.file.write_all(b"second invocation").expect("写入第二个暂存文件");

        first.remove().expect("只删除首个暂存文件");
        let mut remaining = Vec::new();
        second
            .open_read()
            .expect("第二个暂存仍存在")
            .read_to_end(&mut remaining)
            .expect("读取第二个暂存");
        assert_eq!(remaining, b"second invocation");

        drop(first);
        drop(second);
        drop(directory);
        fs::remove_dir_all(directory_path).expect("清理测试目录");
    }
}
