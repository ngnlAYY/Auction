//! 把已核对的 build 安装成只读目录：先写同级暂存目录，封存权限后整体 `rename` 到位。
//!
//! 不变量：build 目录要么完整存在（文件 0444、目录 0555、清单齐全），要么根本不存在：`rename` 之前
//! 暂存目录内每个新建目录与文件都已 `fsync`，所以断电后不会出现缺文件的 build；失败时
//! 暂存目录被清除，入口指针从不在这里改动。已存在的同名 build 只在逐项核对完全一致时视为成功。

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::Context;
use uuid::Uuid;

use super::layout::{
    BuildManifest, FILES_DIR, MANIFEST_FILE, MAX_FILE_BYTES, MAX_FILES, MAX_MANIFEST_BYTES,
    MAX_TOTAL_BYTES, ManifestFile, Stack, Surface, validate_build_id,
};
use super::verify::{require_real_directory, verify_build_dir};
use crate::support::files;

/// 待安装的一个文件；`bytes` 已在调用方核对过摘要。
pub(super) struct InstallFile {
    pub(super) path: String,
    pub(super) sha256: String,
    pub(super) brotli_entity: bool,
    pub(super) bytes: Vec<u8>,
}

/// 待安装的一个 build。
pub(super) struct NewBuild {
    pub(super) surface: Surface,
    pub(super) build_id: String,
    pub(super) stack: Stack,
    pub(super) payload_sha256: String,
    pub(super) files: Vec<InstallFile>,
}

impl NewBuild {
    pub(super) fn manifest(&self) -> BuildManifest {
        let mut files: Vec<ManifestFile> = self
            .files
            .iter()
            .map(|file| ManifestFile {
                path: file.path.clone(),
                sha256: file.sha256.clone(),
                brotli_entity: file.brotli_entity,
            })
            .collect();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        BuildManifest {
            surface: self.surface,
            build_id: self.build_id.clone(),
            stack: self.stack,
            payload_sha256: self.payload_sha256.clone(),
            files,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Installed {
    Created,
    /// 同名 build 已存在且与待安装内容逐项一致。
    AlreadyPresent,
}

/// `web_root` 必须是真实目录；返回（必要时新建的）`<web_root>/<surface>`。
pub(super) fn ensure_surface_root(
    web_root: &Path,
    surface: Surface,
) -> anyhow::Result<std::path::PathBuf> {
    require_real_directory(web_root)?;
    let surface_root = web_root.join(surface.as_str());
    if fs::symlink_metadata(&surface_root).is_err() {
        DirBuilder::new()
            .mode(0o755)
            .create(&surface_root)
            .with_context(|| format!("创建 surface 目录失败：{}", surface_root.display()))?;
        fs::set_permissions(&surface_root, fs::Permissions::from_mode(0o755))
            .context("设置 surface 目录权限失败")?;
    }
    require_real_directory(&surface_root)?;
    Ok(surface_root)
}

/// 安装一个 build；调用前内存里的字节已逐个核对，这里只负责“要么完整、要么不存在”。
pub(super) fn install_build(web_root: &Path, build: &NewBuild) -> anyhow::Result<Installed> {
    validate_build_id(&build.build_id)?;
    enforce_limits(build)?;
    let surface_root = ensure_surface_root(web_root, build.surface)?;
    let target = surface_root.join(&build.build_id);
    let manifest = build.manifest();
    let manifest_text = manifest.render();
    // 与服务端挂载加载器读清单的上限一致：超限的清单装上去服务启动时才失败。
    if manifest_text.len() as u64 > MAX_MANIFEST_BYTES {
        anyhow::bail!("清单超过上限 {MAX_MANIFEST_BYTES} 字节：{}", build.build_id);
    }
    if fs::symlink_metadata(&target).is_ok() {
        let existing = verify_build_dir(&surface_root, build.surface, &build.build_id)
            .context("同名 build 已存在但核对失败，拒绝覆盖")?;
        if existing.manifest != manifest {
            anyhow::bail!("同名 build 已存在且清单不同，拒绝覆盖：{}", build.build_id);
        }
        return Ok(Installed::AlreadyPresent);
    }

    let staging = surface_root.join(format!(".staging-{}-{}", build.build_id, Uuid::new_v4()));
    if let Err(error) = write_and_seal(&staging, build, &manifest_text) {
        return Err(remove_staging(&staging, error));
    }
    if let Err(error) = fs::rename(&staging, &target)
        .with_context(|| format!("安装 build 目录失败：{}", target.display()))
    {
        return Err(remove_staging(&staging, error));
    }
    files::sync_directory(&surface_root, "surface 目录")?;
    Ok(Installed::Created)
}

/// 与服务端挂载加载器同一组上限：超过的 build 装上去也会让服务启动失败，所以安装时就拒绝。
fn enforce_limits(build: &NewBuild) -> anyhow::Result<()> {
    if build.files.len() > MAX_FILES {
        anyhow::bail!("build 文件数超过上限 {MAX_FILES}");
    }
    let mut total = 0_u64;
    for file in &build.files {
        let length = file.bytes.len() as u64;
        if length > MAX_FILE_BYTES {
            anyhow::bail!("单个文件超过上限 {MAX_FILE_BYTES} 字节：{}", file.path);
        }
        total += length;
    }
    if total > MAX_TOTAL_BYTES {
        anyhow::bail!("build 总字节超过上限 {MAX_TOTAL_BYTES}");
    }
    Ok(())
}

fn write_and_seal(staging: &Path, build: &NewBuild, manifest_text: &str) -> anyhow::Result<()> {
    DirBuilder::new().mode(0o755).create(staging).context("创建暂存目录失败")?;
    let files_root = staging.join(FILES_DIR);
    DirBuilder::new().mode(0o755).create(&files_root).context("创建 files 目录失败")?;
    for file in &build.files {
        let path = files_root.join(&file.path);
        if let Some(parent) = path.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o755)
                .create(parent)
                .context("创建子目录失败")?;
        }
        write_readonly(&path, &file.bytes)?;
    }
    write_readonly(&staging.join(MANIFEST_FILE), manifest_text.as_bytes())?;
    seal_directories(&files_root)?;
    fs::set_permissions(staging, fs::Permissions::from_mode(0o555)).context("封存暂存目录失败")?;
    sync_directories(staging)
}

fn write_readonly(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o444)
        .open(path)
        .with_context(|| format!("创建文件失败：{}", path.display()))?;
    file.write_all(bytes).with_context(|| format!("写入文件失败：{}", path.display()))?;
    file.sync_all().with_context(|| format!("同步文件失败：{}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o444)).context("设置只读权限失败")?;
    Ok(())
}

/// 后序对暂存目录树内每个目录 `fsync`（含 `staging` 自身），使新建的目录项先于 `rename` 落盘。
fn sync_directories(directory: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(directory).context("读取目录失败")? {
        let entry = entry.context("读取目录项失败")?;
        if entry.file_type().context("读取文件类型失败")?.is_dir() {
            sync_directories(&entry.path())?;
        }
    }
    files::sync_directory(directory, "暂存目录")
}

/// 后序把目录收紧为 0555（父目录最后），文件已是 0444。
fn seal_directories(directory: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(directory).context("读取目录失败")? {
        let entry = entry.context("读取目录项失败")?;
        if entry.file_type().context("读取文件类型失败")?.is_dir() {
            seal_directories(&entry.path())?;
        }
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o555)).context("封存目录失败")
}

/// 失败后清除暂存目录；清理本身失败时把两个错误都带回去。
fn remove_staging(staging: &Path, cause: anyhow::Error) -> anyhow::Error {
    match unseal_and_remove(staging) {
        Ok(()) => cause,
        Err(cleanup) => cause.context(format!("且清理暂存目录失败：{cleanup:#}")),
    }
}

fn unseal_and_remove(path: &Path) -> anyhow::Result<()> {
    if fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    unseal(path)?;
    fs::remove_dir_all(path).with_context(|| format!("删除暂存目录失败：{}", path.display()))
}

fn unseal(directory: &Path) -> anyhow::Result<()> {
    fs::set_permissions(directory, fs::Permissions::from_mode(0o755))
        .context("放开目录权限失败")?;
    for entry in fs::read_dir(directory).context("读取目录失败")? {
        let entry = entry.context("读取目录项失败")?;
        if entry.file_type().context("读取文件类型失败")?.is_dir() {
            unseal(&entry.path())?;
        }
    }
    Ok(())
}
