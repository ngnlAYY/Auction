//! 备份范围的来源规划与有界收集；只接受普通文件，拒绝符号链接、特殊文件与越界路径名。

use std::{
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

use anyhow::Context;
use auction_config::AppConfig;
use serde::{Deserialize, Serialize};

use crate::{
    backup::{manifest::sha256_bytes, scheduled::filesystem::FileIdentity},
    support::files,
};

/// 单个证据文件的硬上限；邮件诊断证据与配置/回执都远小于此值。
pub(crate) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// 相对路径上限；过长名称进入打包或解包路径容易触发平台差异。
pub(crate) const MAX_RELATIVE_BYTES: usize = 512;
const MAX_DEPTH: usize = 8;
pub(crate) const MAX_FILES_LIMIT: u32 = 8192;
pub(crate) const MIN_TOTAL_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// 三类来源的固定顺序；决定打包与解包的稳定顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    RuntimeConfig,
    MailDiagnostics,
    DeploymentReceipts,
}

impl SourceKind {
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::RuntimeConfig => "runtime_config",
            Self::MailDiagnostics => "mail_diagnostics",
            Self::DeploymentReceipts => "deployment_receipts",
        }
    }
}

/// 一个来源的规划：配置文件逐个列出，其余来源按目录递归收集。
pub(crate) enum SourceSpec {
    Files { kind: SourceKind, paths: Vec<PathBuf> },
    Directory { kind: SourceKind, root: PathBuf, missing_ok: bool },
}

impl SourceSpec {
    fn kind(&self) -> SourceKind {
        match self {
            Self::Files { kind, .. } | Self::Directory { kind, .. } => *kind,
        }
    }
}

pub(crate) struct Plan {
    pub(crate) sources: Vec<SourceSpec>,
    pub(crate) max_files: u32,
    pub(crate) maximum_total_bytes: u64,
}

/// 待打包的单个文件；`path` 已是 `<kind>/<相对路径>`，不包含绝对目录。
#[derive(Debug, Clone)]
pub(crate) struct Artifact {
    pub(crate) kind: SourceKind,
    pub(crate) path: String,
    pub(crate) source: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) identity: FileIdentity,
    modified_seconds: i64,
    modified_nanos: i64,
}

/// 来源投影：只记录摘要、存在性和规模，不记录绝对路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceSummary {
    pub(crate) kind: SourceKind,
    pub(crate) present: bool,
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) root_sha256: Option<String>,
}

pub(crate) struct Collected {
    pub(crate) artifacts: Vec<Artifact>,
    pub(crate) summaries: Vec<SourceSummary>,
}

/// 从已加载的 Worker 配置投影来源；关闭范围时返回 None。
pub(crate) fn plan(config: &AppConfig) -> anyhow::Result<Option<Plan>> {
    let scope = &config.backup.scope;
    if !scope.enabled {
        return Ok(None);
    }
    if !(1..=MAX_FILES_LIMIT).contains(&scope.max_files)
        || !(MIN_TOTAL_BYTES..=MAX_TOTAL_BYTES).contains(&scope.maximum_total_bytes)
    {
        anyhow::bail!("备份范围的 max_files 或 maximum_total_bytes 超出允许范围");
    }
    let mut sources = Vec::new();
    if scope.runtime_config {
        let mut paths = Vec::new();
        for path in [
            config.config_path.as_ref(),
            config.common_config_path.as_ref(),
            config.secrets_path.as_ref(),
            config.runtime_config_path.as_ref(),
            config.worker_config_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !paths.iter().any(|existing| existing == path) {
                paths.push(path.clone());
            }
        }
        if paths.is_empty() {
            anyhow::bail!("备份范围要包含运行配置，但当前配置没有可记录的来源文件");
        }
        sources.push(SourceSpec::Files { kind: SourceKind::RuntimeConfig, paths });
    }
    if scope.mail_diagnostics {
        let directory = config.worker.mooglemail.diagnostics.directory.clone();
        if directory.as_os_str().is_empty() {
            anyhow::bail!("备份范围要包含邮件诊断证据，但诊断目录未配置");
        }
        validate_absolute(&directory)?;
        sources.push(SourceSpec::Directory {
            kind: SourceKind::MailDiagnostics,
            root: directory,
            missing_ok: true,
        });
    }
    if !scope.deployment_receipts_dir.as_os_str().is_empty() {
        validate_absolute(&scope.deployment_receipts_dir)?;
        sources.push(SourceSpec::Directory {
            kind: SourceKind::DeploymentReceipts,
            root: scope.deployment_receipts_dir.clone(),
            missing_ok: false,
        });
    }
    if sources.is_empty() {
        anyhow::bail!("备份范围已启用，但没有可用来源");
    }
    Ok(Some(Plan {
        sources,
        max_files: scope.max_files,
        maximum_total_bytes: scope.maximum_total_bytes,
    }))
}

pub(crate) fn collect(plan: &Plan) -> anyhow::Result<Collected> {
    let mut artifacts = Vec::new();
    let mut summaries = Vec::new();
    let mut total = 0_u64;
    let mut sources = plan.sources.iter().collect::<Vec<_>>();
    sources.sort_by_key(|source| source.kind());
    for source in sources {
        match source {
            SourceSpec::Files { kind, paths } => {
                let mut files = paths.clone();
                files.sort_by(|left, right| {
                    left.as_os_str().as_encoded_bytes().cmp(right.as_os_str().as_encoded_bytes())
                });
                let bytes = collect_files(*kind, &files, plan, &mut artifacts, &mut total)?;
                summaries.push(SourceSummary {
                    kind: *kind,
                    present: true,
                    files: files.len() as u64,
                    bytes,
                    root_sha256: None,
                });
            }
            SourceSpec::Directory { kind, root, missing_ok } => {
                let present = match fs::symlink_metadata(root) {
                    Ok(metadata) => {
                        if !metadata.is_dir() || metadata.file_type().is_symlink() {
                            anyhow::bail!("备份范围来源必须是真实目录：{}", root.display());
                        }
                        true
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound && *missing_ok => {
                        false
                    }
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("读取备份范围来源失败：{}", root.display()));
                    }
                };
                let mut bytes = 0_u64;
                if present {
                    walk(root, *kind, root, 0, plan, &mut artifacts, &mut total, &mut bytes)?;
                }
                summaries.push(SourceSummary {
                    kind: *kind,
                    present,
                    files: artifacts.iter().filter(|artifact| artifact.kind == *kind).count()
                        as u64,
                    bytes,
                    root_sha256: Some(sha256_bytes(root.as_os_str().as_encoded_bytes())),
                });
            }
        }
    }
    enforce_file_count(&artifacts, plan.max_files)?;
    artifacts.sort_by(|left, right| {
        (left.kind, left.path.as_bytes()).cmp(&(right.kind, right.path.as_bytes()))
    });
    for pair in artifacts.windows(2) {
        if pair[0].path == pair[1].path {
            anyhow::bail!("备份范围包含重复的打包路径：{}", pair[0].path);
        }
    }
    Ok(Collected { artifacts, summaries })
}

fn collect_files(
    kind: SourceKind,
    paths: &[PathBuf],
    plan: &Plan,
    artifacts: &mut Vec<Artifact>,
    total: &mut u64,
) -> anyhow::Result<u64> {
    let mut bytes = 0_u64;
    for path in paths {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("备份范围配置文件必须有 UTF-8 文件名")?;
        validate_component(name)?;
        let artifact = artifact(kind, &format!("{}/{name}", kind.slug()), path, plan, total)?;
        bytes = bytes.checked_add(artifact.bytes).context("备份范围字节数溢出")?;
        artifacts.push(artifact);
    }
    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    root: &Path,
    kind: SourceKind,
    directory: &Path,
    depth: usize,
    plan: &Plan,
    artifacts: &mut Vec<Artifact>,
    total: &mut u64,
    bytes: &mut u64,
) -> anyhow::Result<()> {
    if depth > MAX_DEPTH {
        anyhow::bail!("备份范围目录层级超过 {MAX_DEPTH}：{}", directory.display());
    }
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("枚举备份范围目录失败：{}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("枚举备份范围目录失败：{}", directory.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let prefix = directory
        .strip_prefix(root)
        .context("备份范围目录不在来源根下")?
        .to_string_lossy()
        .into_owned();
    for entry in entries {
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(os) => anyhow::bail!("备份范围文件名必须是 UTF-8：{}", os.to_string_lossy()),
        };
        validate_component(&name)?;
        let path = entry.path();
        let relative = if prefix.is_empty() {
            format!("{}/{name}", kind.slug())
        } else {
            format!("{}/{prefix}/{name}", kind.slug())
        };
        if relative.len() > MAX_RELATIVE_BYTES {
            anyhow::bail!("备份范围相对路径超过 {MAX_RELATIVE_BYTES} 字节：{relative}");
        }
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("读取备份范围文件失败：{}", path.display()))?;
        if metadata.file_type().is_symlink() {
            anyhow::bail!("备份范围不得包含符号链接：{}", path.display());
        }
        if metadata.is_dir() {
            walk(root, kind, &path, depth + 1, plan, artifacts, total, bytes)?;
            continue;
        }
        if !metadata.is_file() {
            anyhow::bail!("备份范围不得包含特殊文件：{}", path.display());
        }
        let artifact = artifact(kind, &relative, &path, plan, total)?;
        *bytes = bytes.checked_add(artifact.bytes).context("备份范围字节数溢出")?;
        artifacts.push(artifact);
    }
    Ok(())
}

fn artifact(
    kind: SourceKind,
    relative: &str,
    path: &Path,
    plan: &Plan,
    total: &mut u64,
) -> anyhow::Result<Artifact> {
    let file = files::open_regular(path)?;
    let metadata = file.metadata().context("读取备份范围文件元数据失败")?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
        anyhow::bail!("备份范围文件必须是 owner-only 单链接普通文件：{}", path.display());
    }
    if metadata.len() > MAX_FILE_BYTES {
        anyhow::bail!("备份范围单文件超过 {MAX_FILE_BYTES} 字节：{}", path.display());
    }
    *total = total.checked_add(metadata.len()).context("备份范围字节数溢出")?;
    if *total > plan.maximum_total_bytes {
        anyhow::bail!("备份范围超过 {} 字节上限", plan.maximum_total_bytes);
    }
    Ok(Artifact {
        kind,
        path: relative.to_owned(),
        source: path.to_owned(),
        bytes: metadata.len(),
        identity: FileIdentity::of(&metadata),
        modified_seconds: metadata.mtime(),
        modified_nanos: metadata.mtime_nsec(),
    })
}

/// 在打包前重新确认文件没有被替换或改写；返回的文件描述符用于同一次读取。
pub(crate) fn open_verified(artifact: &Artifact) -> anyhow::Result<File> {
    let file = files::open_regular(&artifact.source)?;
    let metadata = file.metadata().context("读取备份范围文件元数据失败")?;
    if FileIdentity::of(&metadata) != artifact.identity
        || metadata.mtime() != artifact.modified_seconds
        || metadata.mtime_nsec() != artifact.modified_nanos
    {
        anyhow::bail!("备份范围文件在收集后被改写：{}", artifact.path);
    }
    Ok(file)
}

/// 相对路径按段校验：拒绝空段、`.`、`..`、分隔符、控制字符与非 ASCII 反斜杠。
pub(crate) fn validate_relative(path: &str) -> anyhow::Result<()> {
    if path.is_empty() || path.len() > MAX_RELATIVE_BYTES {
        anyhow::bail!("备份范围相对路径无效");
    }
    for part in path.split('/') {
        validate_component(part)?;
    }
    Ok(())
}

fn validate_component(name: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(char::is_control)
        || name.trim() != name
    {
        anyhow::bail!("备份范围名称无效：{name:?}");
    }
    Ok(())
}

/// 允许的绝对路径：无 `..`、无控制字符，且最后一段是正常名称。
pub(crate) fn validate_absolute(input: &Path) -> anyhow::Result<()> {
    if !input.is_absolute()
        || input.components().any(|part| matches!(part, Component::ParentDir))
        || input.to_string_lossy().chars().any(char::is_control)
        || input.file_name().is_none()
    {
        anyhow::bail!("备份范围来源必须是带名称的绝对路径：{}", input.display());
    }
    Ok(())
}

/// 计数上限在收集结束后核对，避免把目录遍历中途的错误吞掉。
pub(crate) fn enforce_file_count(artifacts: &[Artifact], max_files: u32) -> anyhow::Result<()> {
    if artifacts.len() as u64 > u64::from(max_files) {
        anyhow::bail!("备份范围文件数超过 {max_files} 上限");
    }
    Ok(())
}
