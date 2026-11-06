//! 部署制品校验：只接受解压后的 CI 发布包，静态核对文件白名单、权限、摘要与 Web payload
//! 标记。全部检查离线进行，不执行制品内的任何程序，也不连接数据库。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
};

use anyhow::Context;
use sha2::{Digest, Sha256};

/// 分体发布包必须包含的四个程序；角色程序与 `auction-cli` 同属一个制品。
pub(crate) const REQUIRED_BINARIES: [&str; 4] =
    ["auction-worker", "auction-admin-api", "auction-public-api", "auction-cli"];

/// `RELEASE-MANIFEST.v1` 与 `SHA256SUMS` 的读取上限。
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// 单个文件大小上限；制品二进制远小于此值，超限说明不是发布包。
const MAX_MEMBER_BYTES: u64 = 512 * 1024 * 1024;
/// 制品文件数量上限，避免在异常目录树上散列无限文件。
const MAX_MEMBERS: usize = 4096;

const RELEASE_MANIFEST: &str = "RELEASE-MANIFEST.v1";
const RELEASE_MANIFEST_SCHEMA: &str = "auction-release-manifest/1";
const CHECKSUMS: &str = "SHA256SUMS";
const DEPENDENCIES: &str = "docs/DEPENDENCIES.md";
const WEB_MANIFEST_SCHEMA: &str = "auction-web-assets/1";

/// 通过静态校验的部署制品；只保留路径、标识与摘要。
#[derive(Debug, Clone)]
pub(crate) struct Artifact {
    pub(crate) root: PathBuf,
    pub(crate) package: String,
    pub(crate) source_commit: String,
    pub(crate) source_fingerprint: String,
    pub(crate) binaries: Vec<ArtifactBinary>,
    pub(crate) public_payload_sha256: String,
    pub(crate) admin_payload_sha256: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ArtifactBinary {
    pub(crate) name: &'static str,
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
    if !root.is_absolute() {
        anyhow::bail!("制品目录必须是绝对路径：{}", root.display());
    }
    let metadata = fs::symlink_metadata(root)
        .with_context(|| format!("读取制品目录失败：{}", root.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("制品必须是解压后的普通目录：{}", root.display());
    }

    let manifest_bytes = read_bounded(&root.join(RELEASE_MANIFEST), MAX_MANIFEST_BYTES)?;
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

    let checksums_bytes = read_bounded(&root.join(CHECKSUMS), MAX_MANIFEST_BYTES)?;
    let checksums = parse_sha256sums(&checksums_bytes).context("解析 SHA256SUMS 失败")?;
    // 发布脚本在同一文件内写入自身摘要，实际值取决于写入顺序；该条只参与集合校验，不参与比对。
    for path in manifest.files.keys().filter(|path| path.as_str() != CHECKSUMS) {
        let expected =
            checksums.get(path).with_context(|| format!("SHA256SUMS 缺少清单文件：{path}"))?;
        let (file, _bytes) = read_member(root, path)?;
        if &sha256_reader(&file)? != expected {
            anyhow::bail!("制品摘要与 SHA256SUMS 不一致：{path}");
        }
    }
    let extra: Vec<_> =
        checksums.keys().filter(|path| !manifest.files.contains_key(*path)).cloned().collect();
    if !extra.is_empty() {
        anyhow::bail!("SHA256SUMS 记录了清单之外的文件：{}", extra.join(", "));
    }

    let dependencies_bytes = read_bounded(&root.join(DEPENDENCIES), MAX_MANIFEST_BYTES)?;
    let dependencies =
        std::str::from_utf8(&dependencies_bytes).context("docs/DEPENDENCIES.md 不是 UTF-8")?;
    if !dependencies.contains(&format!("- Source commit：`{}`", manifest.source_commit)) {
        anyhow::bail!("docs/DEPENDENCIES.md 的来源提交与 RELEASE-MANIFEST.v1 不一致");
    }
    if !dependencies.contains("- Source worktree：`clean`") {
        anyhow::bail!("拒绝部署工作树不干净时生成的制品");
    }

    let public_payload = inspect_web_surface(root, "public")?;
    let admin_payload = inspect_web_surface(root, "admin")?;
    if public_payload == admin_payload {
        anyhow::bail!("Public 与 Admin 的 Web payload 摘要不得相同");
    }

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
        binaries.push(ArtifactBinary {
            name,
            path: root.join(&path),
            sha256: digest.clone(),
            bytes: metadata.len(),
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

/// 发布包名只允许发布脚本生成的形状；合并包（`-single`）与脏包（`-dirty`）不能用于分体部署。
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
    {
        anyhow::bail!("RELEASE-MANIFEST.v1 的 package 不是可分体部署的发布包名");
    }
    Ok(())
}

fn collect_members(root: &Path) -> anyhow::Result<BTreeMap<String, u32>> {
    let mut members = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
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
                pending.push(path);
                continue;
            }
            if !metadata.is_file() || metadata.len() > MAX_MEMBER_BYTES {
                anyhow::bail!("制品包含非普通文件或超大文件：{relative}");
            }
            if members.insert(relative.clone(), metadata.permissions().mode() & 0o777).is_some() {
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

/// 校验一个 Web surface 的静态资源标记；返回 `sha256(WEB-<surface>-SHA256)`。
fn inspect_web_surface(root: &Path, surface: &str) -> anyhow::Result<String> {
    let list_path = root.join(format!("docs/WEB-{}-SHA256", surface.to_uppercase()));
    let manifest_path = root.join(format!("docs/WEB-{}-MANIFEST.v1", surface.to_uppercase()));
    let list_bytes = read_bounded(&list_path, MAX_MANIFEST_BYTES)?;
    let list = std::str::from_utf8(&list_bytes).context("Web payload 摘要清单不是 UTF-8")?;
    let mut assets = BTreeSet::new();
    for line in list.lines() {
        let Some((digest, path)) = line.split_once("  ") else {
            anyhow::bail!("Web payload 摘要清单行格式无效：{surface}");
        };
        if !is_lower_hex(digest, 64) || !path.starts_with("./") {
            anyhow::bail!("Web payload 摘要清单行格式无效：{surface}");
        }
        validate_relative_path(&path[2..])?;
        if !assets.insert(path.to_owned()) {
            anyhow::bail!("Web payload 摘要清单重复记录：{surface} {path}");
        }
    }
    if assets.is_empty() {
        anyhow::bail!("Web payload 摘要清单为空：{surface}");
    }
    let payload_sha256 = sha256_bytes(&list_bytes);

    let manifest_bytes = read_bounded(&manifest_path, MAX_MANIFEST_BYTES)?;
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
        || fields.get("profile").is_none_or(String::is_empty)
    {
        anyhow::bail!("Web 资源标记的 schema、surface 或 profile 无效：{surface}");
    }
    if fields.get("payload_sha256").map(String::as_str) != Some(payload_sha256.as_str()) {
        anyhow::bail!("Web 资源标记的 payload_sha256 与摘要清单不一致：{surface}");
    }
    Ok(payload_sha256)
}

fn read_bounded(path: &Path, limit: u64) -> anyhow::Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("打开文件失败：{}", path.display()))?;
    let metadata = file.metadata().context("读取文件元数据失败")?;
    if !metadata.is_file() || metadata.len() > limit {
        anyhow::bail!("文件不是普通文件或超过上限：{}", path.display());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("读取文件失败：{}", path.display()))?;
    if bytes.len() as u64 > limit {
        anyhow::bail!("文件超过 {limit} 字节上限：{}", path.display());
    }
    Ok(bytes)
}

/// 打开并读取成员文件；返回描述符与字节数，供摘要计算与统计复用。
fn read_member(root: &Path, relative: &str) -> anyhow::Result<(File, u64)> {
    let path = root.join(relative);
    let metadata =
        fs::symlink_metadata(&path).with_context(|| format!("读取制品文件失败：{relative}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("制品成员不是普通文件：{relative}");
    }
    if metadata.len() > MAX_MEMBER_BYTES {
        anyhow::bail!("制品成员超过大小上限：{relative}");
    }
    Ok((
        File::open(&path).with_context(|| format!("打开制品文件失败：{relative}"))?,
        metadata.len(),
    ))
}

fn sha256_reader(file: &File) -> anyhow::Result<String> {
    let mut reader = file;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).context("读取制品文件失败")?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(lower_hex(&digest.finalize()))
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    lower_hex(&digest.finalize())
}

pub(crate) fn sha256_path(path: &Path) -> anyhow::Result<(String, u64)> {
    let file = File::open(path).with_context(|| format!("打开文件失败：{}", path.display()))?;
    let bytes = file.metadata().context("读取文件元数据失败")?.len();
    Ok((sha256_reader(&file)?, bytes))
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(HEX[usize::from(byte >> 4)] as char);
        value.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    value
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
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
mod tests {
    use std::{
        fs,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
        path::{Path, PathBuf},
    };

    use super::{CHECKSUMS, inspect, sha256_bytes};

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("auction-artifact-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).expect("建立临时制品目录");
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, bytes: &[u8], mode: u32) {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("建立目录");
            }
            {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(mode)
                    .open(&path)
                    .expect("创建文件");
                std::io::Write::write_all(&mut file, bytes).expect("写入文件");
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("设置权限");
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn web_list(seed: &str) -> Vec<u8> {
        format!(
            "{}  ./assets/app.js\n{}  ./index.html\n",
            "a".repeat(63) + seed,
            "b".repeat(63) + seed
        )
        .into_bytes()
    }

    /// 生成一个自洽的最小发布包；每个文件都进入清单与摘要（SHA256SUMS 自身除外）。
    fn package() -> TempRoot {
        let root = TempRoot::new();
        let files: Vec<(&str, Vec<u8>, u32)> = vec![
            ("bin/auction-worker", b"worker".to_vec(), 0o755),
            ("bin/auction-admin-api", b"admin".to_vec(), 0o755),
            ("bin/auction-public-api", b"public".to_vec(), 0o755),
            ("bin/auction-cli", b"cli".to_vec(), 0o755),
        ];
        let public_list = web_list("1");
        let admin_list = web_list("2");
        let public_payload = sha256_bytes(&public_list);
        let admin_payload = sha256_bytes(&admin_list);
        let commit = "a".repeat(40);
        let fingerprint = "b".repeat(64);
        let mut entries = files.clone();
        entries.push(("docs/WEB-PUBLIC-SHA256", public_list, 0o644));
        entries.push(("docs/WEB-ADMIN-SHA256", admin_list, 0o644));
        entries.push((
            "docs/WEB-PUBLIC-MANIFEST.v1",
            format!(
                "schema=auction-web-assets/1\nsurface=public\nprofile=wasm-release\npayload_sha256={public_payload}\n"
            )
            .into_bytes(),
            0o644,
        ));
        entries.push((
            "docs/WEB-ADMIN-MANIFEST.v1",
            format!(
                "schema=auction-web-assets/1\nsurface=admin\nprofile=wasm-release\npayload_sha256={admin_payload}\n"
            )
            .into_bytes(),
            0o644,
        ));
        entries.push((
            "docs/DEPENDENCIES.md",
            format!("- Source commit：`{commit}`\n- Source worktree：`clean`\n").into_bytes(),
            0o644,
        ));
        for (relative, bytes, mode) in &entries {
            root.write(relative, bytes, *mode);
        }
        let mut manifest = format!(
            "schema=auction-release-manifest/1\npackage=auction-rust-0.1.0-{:12}\nsource_commit={commit}\nsource_fingerprint={fingerprint}\nfiles=\n",
            &commit[..12]
        );
        for (relative, _, mode) in &entries {
            manifest.push_str(&format!("{mode:o} {relative}\n"));
        }
        manifest.push_str("644 RELEASE-MANIFEST.v1\n");
        manifest.push_str(&format!("644 {CHECKSUMS}\n"));
        root.write("RELEASE-MANIFEST.v1", manifest.as_bytes(), 0o644);
        let mut sums = String::new();
        for (relative, bytes, _) in &entries {
            sums.push_str(&format!("{}  {relative}\n", sha256_bytes(bytes)));
        }
        sums.push_str(&format!("{}  RELEASE-MANIFEST.v1\n", sha256_bytes(manifest.as_bytes())));
        sums.push_str(&format!("{}  {CHECKSUMS}\n", "0".repeat(64)));
        root.write(CHECKSUMS, sums.as_bytes(), 0o644);
        root
    }

    #[test]
    fn valid_release_package_passes() {
        let root = package();
        let artifact = inspect(root.path()).expect("自洽发布包必须通过");
        assert_eq!(artifact.source_commit, "a".repeat(40));
        assert_ne!(artifact.public_payload_sha256, artifact.admin_payload_sha256);
        assert_eq!(artifact.cli_sha256(), sha256_bytes(b"cli").as_str());
        assert!(artifact.binary("auction-worker").is_some());
    }

    #[test]
    fn mutated_binary_or_missing_file_is_rejected() {
        let root = package();
        root.write("bin/auction-cli", b"tampered", 0o755);
        assert!(inspect(root.path()).is_err());

        let root = package();
        fs::remove_file(root.path().join("docs/WEB-ADMIN-SHA256")).expect("删除标记");
        assert!(inspect(root.path()).is_err());

        let root = package();
        root.write("docs/extra.md", b"extra", 0o644);
        assert!(inspect(root.path()).is_err());
    }

    #[test]
    fn manifest_and_payload_inconsistencies_are_rejected() {
        let root = package();
        root.write("docs/WEB-PUBLIC-MANIFEST.v1", b"schema=auction-web-assets/1\nsurface=public\nprofile=wasm-release\npayload_sha256=0000000000000000000000000000000000000000000000000000000000000000\n", 0o644);
        assert!(inspect(root.path()).is_err());

        let root = package();
        root.write(
            "docs/DEPENDENCIES.md",
            format!("- Source commit：`{}`\n- Source worktree：`dirty`\n", "a".repeat(40))
                .as_bytes(),
            0o644,
        );
        // DEPENDENCIES 变更会同时改变摘要；这里必须重算 SHA256SUMS 才能命中工作树判定。
        let mut sums = String::new();
        for (relative, bytes, _) in members(root.path()) {
            sums.push_str(&format!("{}  {relative}\n", sha256_bytes(&bytes)));
        }
        sums.push_str(&format!("{}  {CHECKSUMS}\n", "0".repeat(64)));
        root.write(CHECKSUMS, sums.as_bytes(), 0o644);
        assert!(inspect(root.path()).is_err());
    }

    fn members(root: &Path) -> Vec<(String, Vec<u8>, u32)> {
        let mut result = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).expect("读取目录").flatten() {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).expect("读取元数据");
                if metadata.is_dir() {
                    pending.push(path);
                    continue;
                }
                let relative =
                    path.strip_prefix(root).expect("相对路径").to_str().expect("UTF-8").to_owned();
                if relative == CHECKSUMS {
                    continue;
                }
                result.push((
                    relative,
                    fs::read(&path).expect("读取文件"),
                    metadata.permissions().mode() & 0o777,
                ));
            }
        }
        result
    }

    #[test]
    fn package_name_and_modes_are_enforced() {
        let root = package();
        let text = fs::read_to_string(root.path().join("RELEASE-MANIFEST.v1"))
            .expect("读取清单")
            .replace("auction-rust-0.1.0-", "auction-rust-0.1.0-single-");
        root.write("RELEASE-MANIFEST.v1", text.as_bytes(), 0o644);
        let mut sums = String::new();
        for (relative, bytes, _) in members(root.path()) {
            sums.push_str(&format!("{}  {relative}\n", sha256_bytes(&bytes)));
        }
        sums.push_str(&format!("{}  {CHECKSUMS}\n", "0".repeat(64)));
        root.write(CHECKSUMS, sums.as_bytes(), 0o644);
        assert!(inspect(root.path()).is_err());
    }

    #[test]
    fn symlinks_are_rejected() {
        let root = package();
        std::os::unix::fs::symlink("/etc/hostname", root.path().join("docs/link"))
            .expect("建立符号链接");
        assert!(inspect(root.path()).is_err());
    }
}
