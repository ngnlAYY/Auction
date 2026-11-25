//! 部署制品校验：只接受解压后的 CI 发布包，静态核对文件白名单、权限、摘要与 Web payload
//! 标记。全部检查离线进行，不执行制品内的任何程序，也不连接数据库。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

use anyhow::Context;

use crate::support::digest::{is_lower_hex, sha256_bytes, sha256_reader};
use crate::support::files;

pub(crate) mod package_receipt;
mod web_stack;

#[cfg(test)]
mod package_receipt_tests;
#[cfg(test)]
mod web_stack_tests;

pub(crate) use web_stack::{FIRST_WEB_STACK_VERSION, WebStackIdentity};

/// 分体发布包必须包含的四个程序；角色程序与 `auction-cli` 同属一个制品。
pub(crate) const REQUIRED_BINARIES: [&str; 4] =
    ["auction-worker", "auction-admin-api", "auction-public-api", "auction-cli"];

/// `RELEASE-MANIFEST.v1` 与 `SHA256SUMS` 的读取上限。
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// 单个文件大小上限；制品二进制远小于此值，超限说明不是发布包。
const MAX_MEMBER_BYTES: u64 = 512 * 1024 * 1024;
/// 制品文件数量上限，避免在异常目录树上散列无限文件。
const MAX_MEMBERS: usize = 4096;
/// 制品目录数量与深度上限，避免异常目录树导致无界枚举或内存增长。
const MAX_DIRECTORIES: usize = 4096;
const MAX_DIRECTORY_DEPTH: usize = 64;

const RELEASE_MANIFEST: &str = "RELEASE-MANIFEST.v1";
const RELEASE_MANIFEST_SCHEMA: &str = "auction-release-manifest/1";
const CHECKSUMS: &str = "SHA256SUMS";
const DEPENDENCIES: &str = "docs/DEPENDENCIES.md";
const WEB_MANIFEST_SCHEMA: &str = "auction-web-assets/1";

/// 通过静态校验的部署制品；除元数据外，为每个二进制保留已校验的打开句柄，避免重开可变路径。
#[derive(Debug)]
pub(crate) struct Artifact {
    pub(crate) root: PathBuf,
    pub(crate) package: String,
    pub(crate) source_commit: String,
    pub(crate) source_fingerprint: String,
    pub(crate) binaries: Vec<ArtifactBinary>,
    pub(crate) public_payload_sha256: String,
    pub(crate) admin_payload_sha256: String,
    /// 新格式包（含 `docs/WEB-STACK.v1`）的页面凭据；旧格式 Rust 包为 `None`。
    pub(crate) web_stack: Option<WebStackIdentity>,
}

/// 一个 Web surface 的摘要清单：清单原字节的摘要与逐文件摘要（路径不含 `./`）。
pub(crate) struct WebSurface {
    pub(crate) payload_sha256: String,
    pub(crate) assets: BTreeMap<String, String>,
}

#[derive(Debug)]
pub(crate) struct ArtifactBinary {
    pub(crate) name: &'static str,
    pub(crate) file: File,
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
}

impl Artifact {
    pub(crate) fn binary(&self, name: &str) -> Option<&ArtifactBinary> {
        self.binaries.iter().find(|binary| binary.name == name)
    }

    pub(crate) fn cli_sha256(&self) -> &str {
        self.binary("auction-cli").map_or("", |binary| binary.sha256.as_str())
    }
}

/// 校验发布包布局；任何缺失、额外文件、权限不符或摘要不符都失败关闭。
pub(crate) fn inspect(root: &Path) -> anyhow::Result<Artifact> {
    inspect_at(root, FIRST_WEB_STACK_VERSION)
}

/// [`inspect`] 的可参数化形式：`first_web_stack_version` 是新格式的版本硬边界。
fn inspect_at(root: &Path, first_web_stack_version: &str) -> anyhow::Result<Artifact> {
    if !root.is_absolute() {
        anyhow::bail!("制品目录必须是绝对路径：{}", root.display());
    }
    let metadata = fs::symlink_metadata(root)
        .with_context(|| format!("读取制品目录失败：{}", root.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("制品必须是解压后的普通目录：{}", root.display());
    }
    let manifest_bytes = files::read_bounded(&root.join(RELEASE_MANIFEST), MAX_MANIFEST_BYTES)?;
    let manifest =
        parse_release_manifest(&manifest_bytes).context("解析 RELEASE-MANIFEST.v1 失败")?;
    let actual = collect_members(root)?;
    if actual.len() != manifest.files.len() {
        anyhow::bail!(
            "制品文件数量 {} 与 RELEASE-MANIFEST.v1 白名单 {} 不一致",
            actual.len(),
            manifest.files.len()
        );
    }
    for (path, mode) in &manifest.files {
        let Some(found) = actual.get(path) else {
            anyhow::bail!("制品缺少清单文件：{path}");
        };
        if found != mode {
            anyhow::bail!("制品文件权限与清单不一致：{path}（{found:o} != {mode:o}）");
        }
    }

    let checksums_bytes = files::read_bounded(&root.join(CHECKSUMS), MAX_MANIFEST_BYTES)?;
    let checksums = parse_sha256sums(&checksums_bytes).context("解析 SHA256SUMS 失败")?;
    // SHA256SUMS 不列入自身摘要；仍须出现在 RELEASE-MANIFEST 白名单中，但不参与摘要比对。
    if checksums.contains_key(CHECKSUMS) {
        anyhow::bail!("SHA256SUMS 不得包含自身摘要");
    }
    for path in manifest.files.keys().filter(|path| path.as_str() != CHECKSUMS) {
        let expected =
            checksums.get(path).with_context(|| format!("SHA256SUMS 缺少清单文件：{path}"))?;
        let (file, _bytes) = read_member(root, path)?;
        let (digest, _) =
            sha256_reader(&file).with_context(|| format!("读取制品文件失败：{path}"))?;
        if &digest != expected {
            anyhow::bail!("制品摘要与 SHA256SUMS 不一致：{path}");
        }
    }
    let extra: Vec<_> =
        checksums.keys().filter(|path| !manifest.files.contains_key(*path)).cloned().collect();
    if !extra.is_empty() {
        anyhow::bail!("SHA256SUMS 记录了清单之外的文件：{}", extra.join(", "));
    }

    let dependencies_bytes = files::read_bounded(&root.join(DEPENDENCIES), MAX_MANIFEST_BYTES)?;
    let dependencies =
        std::str::from_utf8(&dependencies_bytes).context("docs/DEPENDENCIES.md 不是 UTF-8")?;
    if !dependencies.contains(&format!("- Source commit：`{}`", manifest.source_commit)) {
        anyhow::bail!("docs/DEPENDENCIES.md 的来源提交与 RELEASE-MANIFEST.v1 不一致");
    }
    if !dependencies.contains("- Source worktree：`clean`") {
        anyhow::bail!("拒绝部署工作树不干净时生成的制品");
    }

    // 先定页面格式再核标记：新格式按栈取 profile，旧格式固定 Rust 版（wasm-release）。
    let web_stack = web_stack::read_identity(
        root,
        &manifest.package,
        &manifest.source_commit,
        &manifest.source_fingerprint,
        &manifest.files,
        first_web_stack_version,
    )?;
    let public_surface = inspect_web_surface(
        root,
        "public",
        web_stack::surface_profile(web_stack.as_ref(), "public").profile(),
    )?;
    let admin_surface = inspect_web_surface(
        root,
        "admin",
        web_stack::surface_profile(web_stack.as_ref(), "admin").profile(),
    )?;
    if public_surface.payload_sha256 == admin_surface.payload_sha256 {
        anyhow::bail!("Public 与 Admin 的 Web payload 摘要不得相同");
    }
    if let Some(identity) = &web_stack {
        identity.verify_surfaces(&public_surface, &admin_surface, &manifest.files, &checksums)?;
    }
    let (public_payload, admin_payload) =
        (public_surface.payload_sha256, admin_surface.payload_sha256);

    let mut binaries = Vec::with_capacity(REQUIRED_BINARIES.len());
    for name in REQUIRED_BINARIES {
        let path = format!("bin/{name}");
        let mode = manifest.files.get(&path).with_context(|| format!("制品缺少程序：{path}"))?;
        if mode & 0o777 != 0o755 {
            anyhow::bail!("制品程序必须为 0755：{path}");
        }
        let digest =
            checksums.get(&path).with_context(|| format!("SHA256SUMS 缺少程序：{path}"))?;
        let metadata = fs::symlink_metadata(root.join(&path))
            .with_context(|| format!("读取制品程序失败：{path}"))?;
        if metadata.len() == 0 {
            anyhow::bail!("制品程序不得为空：{path}");
        }
        let (mut file, bytes) = read_member(root, &path)?;
        let metadata =
            file.metadata().with_context(|| format!("读取制品程序元数据失败：{path}"))?;
        if metadata.len() == 0 || bytes != metadata.len() {
            anyhow::bail!("制品程序不得为空或读取期间变化：{path}");
        }
        let (actual_digest, hashed_bytes) =
            sha256_reader(&mut file).with_context(|| format!("读取制品程序失败：{path}"))?;
        if actual_digest != *digest || hashed_bytes != bytes {
            anyhow::bail!("制品程序摘要或大小与已校验清单不一致：{path}");
        }
        binaries.push(ArtifactBinary {
            name,
            file,
            path: root.join(&path),
            sha256: digest.clone(),
            bytes,
        });
    }

    Ok(Artifact {
        root: root.to_path_buf(),
        package: manifest.package,
        source_commit: manifest.source_commit,
        source_fingerprint: manifest.source_fingerprint,
        binaries,
        public_payload_sha256: public_payload,
        admin_payload_sha256: admin_payload,
        web_stack,
    })
}

struct ReleaseManifest {
    package: String,
    source_commit: String,
    source_fingerprint: String,
    files: BTreeMap<String, u32>,
}

fn parse_release_manifest(bytes: &[u8]) -> anyhow::Result<ReleaseManifest> {
    let text = std::str::from_utf8(bytes).context("RELEASE-MANIFEST.v1 不是 UTF-8")?;
    let mut head = BTreeMap::new();
    let mut files = BTreeMap::new();
    let mut in_files = false;
    for line in text.lines() {
        if line.is_empty() || line.trim() != line {
            anyhow::bail!("RELEASE-MANIFEST.v1 含空行或首尾空白");
        }
        if in_files {
            let (mode, path) = line
                .split_once(' ')
                .with_context(|| format!("RELEASE-MANIFEST.v1 文件行缺少空格：{line}"))?;
            let mode = parse_mode(mode)?;
            validate_relative_path(path)?;
            if files.insert(path.to_owned(), mode).is_some() {
                anyhow::bail!("RELEASE-MANIFEST.v1 重复记录文件：{path}");
            }
            continue;
        }
        if line == "files=" {
            in_files = true;
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .with_context(|| format!("RELEASE-MANIFEST.v1 行格式无效：{line}"))?;
        if head.insert(key.to_owned(), value.to_owned()).is_some() {
            anyhow::bail!("RELEASE-MANIFEST.v1 重复字段：{key}");
        }
    }
    if !in_files || files.is_empty() {
        anyhow::bail!("RELEASE-MANIFEST.v1 缺少 files 段");
    }
    let allowed: BTreeSet<&str> =
        ["schema", "package", "source_commit", "source_fingerprint"].into_iter().collect();
    if head.keys().any(|key| !allowed.contains(key.as_str())) {
        anyhow::bail!("RELEASE-MANIFEST.v1 含未知字段");
    }
    if head.get("schema").map(String::as_str) != Some(RELEASE_MANIFEST_SCHEMA) {
        anyhow::bail!("RELEASE-MANIFEST.v1 的 schema 不受支持");
    }
    let package = head.get("package").context("RELEASE-MANIFEST.v1 缺少 package")?.clone();
    validate_package_name(&package)?;
    let source_commit =
        head.get("source_commit").context("RELEASE-MANIFEST.v1 缺少 source_commit")?.clone();
    if !is_lower_hex(&source_commit, 40) {
        anyhow::bail!("RELEASE-MANIFEST.v1 的 source_commit 无效");
    }
    let source_fingerprint = head
        .get("source_fingerprint")
        .context("RELEASE-MANIFEST.v1 缺少 source_fingerprint")?
        .clone();
    if !is_lower_hex(&source_fingerprint, 64) {
        anyhow::bail!("RELEASE-MANIFEST.v1 的 source_fingerprint 无效");
    }
    Ok(ReleaseManifest { package, source_commit, source_fingerprint, files })
}

/// 正式分体发布包名才可部署；本地、分支和合并候选包不能进入部署流程。
fn validate_package_name(package: &str) -> anyhow::Result<()> {
    if package.len() > 256
        || package.contains('/')
        || package.chars().any(char::is_control)
        || package.trim() != package
        || !package.starts_with("auction-rust-")
        || package.ends_with("-dirty")
        || package.contains("-dirty-")
        || package.ends_with("-single")
        || package.contains("-single-")
        || package.ends_with("-candidate")
    {
        anyhow::bail!("RELEASE-MANIFEST.v1 的 package 不是可分体部署的发布包名");
    }
    Ok(())
}

fn collect_members(root: &Path) -> anyhow::Result<BTreeMap<String, u32>> {
    let mut members = BTreeMap::new();
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut directory_count = 1_usize;
    while let Some((directory, depth)) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("读取制品目录失败：{}", directory.display()))?
        {
            let entry = entry.context("读取制品目录项失败")?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("读取制品文件失败：{}", path.display()))?;
            if metadata.file_type().is_symlink() {
                anyhow::bail!("制品不得包含符号链接：{}", path.display());
            }
            let relative = path
                .strip_prefix(root)
                .context("制品路径越出根目录")?
                .to_str()
                .context("制品文件路径不是 UTF-8")?
                .to_owned();
            validate_relative_path(&relative)?;
            if metadata.is_dir() {
                directory_count += 1;
                let child_depth = depth + 1;
                if directory_count > MAX_DIRECTORIES {
                    anyhow::bail!("制品目录数量超过 {MAX_DIRECTORIES} 上限");
                }
                if child_depth > MAX_DIRECTORY_DEPTH {
                    anyhow::bail!("制品目录深度超过 {MAX_DIRECTORY_DEPTH} 上限");
                }
                pending.push((path, child_depth));
                continue;
            }
            if !metadata.is_file()
                || metadata.len() > MAX_MEMBER_BYTES
                || metadata.permissions().mode() & 0o7000 != 0
            {
                anyhow::bail!("制品包含非普通文件、特殊权限位或超大文件：{relative}");
            }
            if members.insert(relative.clone(), metadata.permissions().mode() & 0o7777).is_some() {
                anyhow::bail!("制品包含重复路径：{relative}");
            }
            if members.len() > MAX_MEMBERS {
                anyhow::bail!("制品文件数量超过 {MAX_MEMBERS} 上限");
            }
        }
    }
    Ok(members)
}

fn parse_sha256sums(bytes: &[u8]) -> anyhow::Result<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).context("SHA256SUMS 不是 UTF-8")?;
    let mut sums = BTreeMap::new();
    for line in text.lines() {
        let Some((digest, path)) = line.split_once("  ") else {
            anyhow::bail!("SHA256SUMS 行格式无效：{line}");
        };
        if !is_lower_hex(digest, 64) || path.contains("  ") {
            anyhow::bail!("SHA256SUMS 行格式无效：{line}");
        }
        validate_relative_path(path)?;
        if sums.insert(path.to_owned(), digest.to_owned()).is_some() {
            anyhow::bail!("SHA256SUMS 重复记录：{path}");
        }
    }
    if sums.is_empty() {
        anyhow::bail!("SHA256SUMS 为空");
    }
    Ok(sums)
}

/// 校验一个 Web surface 的构建清单与标记（`profile` 须等于包格式所定的值）并返回清单摘要。
/// 旧格式的 Web 文件已嵌入对应 surface 二进制，发布包仅携带摘要清单与标记；新格式另附导出树，
/// 由 [`web_stack`] 逐文件核对。
fn inspect_web_surface(root: &Path, surface: &str, profile: &str) -> anyhow::Result<WebSurface> {
    let list_path = root.join(format!("docs/WEB-{}-SHA256", surface.to_uppercase()));
    let manifest_path = root.join(format!("docs/WEB-{}-MANIFEST.v1", surface.to_uppercase()));
    let list_bytes = files::read_bounded(&list_path, MAX_MANIFEST_BYTES)?;
    let list = std::str::from_utf8(&list_bytes).context("Web payload 摘要清单不是 UTF-8")?;
    let mut assets = BTreeMap::new();
    for line in list.lines() {
        let Some((digest, path)) = line.split_once("  ") else {
            anyhow::bail!("Web payload 摘要清单行格式无效：{surface}");
        };
        if !is_lower_hex(digest, 64) || !path.starts_with("./") {
            anyhow::bail!("Web payload 摘要清单行格式无效：{surface}");
        }
        validate_relative_path(&path[2..])?;
        if assets.insert(path[2..].to_owned(), digest.to_owned()).is_some() {
            anyhow::bail!("Web payload 摘要清单重复记录：{surface} {path}");
        }
    }
    if assets.is_empty() {
        anyhow::bail!("Web payload 摘要清单为空：{surface}");
    }
    let payload_sha256 = sha256_bytes(&list_bytes);

    let manifest_bytes = files::read_bounded(&manifest_path, MAX_MANIFEST_BYTES)?;
    let manifest = std::str::from_utf8(&manifest_bytes).context("Web 资源标记不是 UTF-8")?;
    let mut fields = BTreeMap::new();
    for line in manifest.lines() {
        let (key, value) =
            line.split_once('=').with_context(|| format!("Web 资源标记行格式无效：{surface}"))?;
        if fields.insert(key.to_owned(), value.to_owned()).is_some() {
            anyhow::bail!("Web 资源标记重复字段：{surface} {key}");
        }
    }
    let allowed: BTreeSet<&str> =
        ["schema", "surface", "profile", "payload_sha256"].into_iter().collect();
    if fields.keys().any(|key| !allowed.contains(key.as_str())) {
        anyhow::bail!("Web 资源标记含未知字段：{surface}");
    }
    if fields.get("schema").map(String::as_str) != Some(WEB_MANIFEST_SCHEMA)
        || fields.get("surface").map(String::as_str) != Some(surface)
        || fields.get("profile").map(String::as_str) != Some(profile)
    {
        anyhow::bail!("Web 资源标记的 schema、surface 或部署 profile 无效：{surface}");
    }
    if fields.get("payload_sha256").map(String::as_str) != Some(payload_sha256.as_str()) {
        anyhow::bail!("Web 资源标记的 payload_sha256 与摘要清单不一致：{surface}");
    }
    Ok(WebSurface { payload_sha256, assets })
}

/// 打开并读取成员文件；返回描述符与字节数，供摘要计算与统计复用。
fn read_member(root: &Path, relative: &str) -> anyhow::Result<(File, u64)> {
    let path = root.join(relative);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&path)
        .with_context(|| format!("打开制品文件失败：{relative}"))?;
    let metadata =
        file.metadata().with_context(|| format!("读取制品文件元数据失败：{relative}"))?;
    if !metadata.is_file() {
        anyhow::bail!("制品成员不是普通文件：{relative}");
    }
    if metadata.len() > MAX_MEMBER_BYTES {
        anyhow::bail!("制品成员超过大小上限：{relative}");
    }
    let bytes = metadata.len();
    Ok((file, bytes))
}

fn parse_mode(value: &str) -> anyhow::Result<u32> {
    if !(3..=4).contains(&value.len()) || !value.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
        anyhow::bail!("RELEASE-MANIFEST.v1 的权限无效：{value}");
    }
    Ok(u32::from_str_radix(value, 8)?)
}

/// 相对路径必须是普通组件：不含绝对前缀、`..`、反斜杠、控制字符或空段。
fn validate_relative_path(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 4096
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        anyhow::bail!("制品路径无效：{value}");
    }
    let path = Path::new(value);
    if !path.components().all(|component| matches!(component, Component::Normal(_))) {
        anyhow::bail!("制品路径必须只含普通组件：{value}");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
