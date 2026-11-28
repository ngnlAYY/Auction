//! 备份范围的来源规划与有界收集；只接受普通文件，拒绝符号链接、特殊文件与越界路径名。

use std::{
    ffi::CString,
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use anyhow::Context;
use auction_config::AppConfig;
use serde::{Deserialize, Serialize};

use self::directory::{open_child_file, read_directory_names};
use self::path_validation::validate_component;
use crate::support::digest::sha256_bytes;
use crate::support::files::{Facts, FileIdentity, Kind};

// 解包输出目录与来源目录共用同一祖先信任口径。
pub(crate) use self::directory::open_directory;

mod directory;
mod path_validation;

pub(crate) use self::path_validation::validate_relative;

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
    parent: Arc<File>,
    name: CString,
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
        if directory.is_empty() {
            anyhow::bail!("备份范围要包含邮件诊断证据，但诊断目录未配置");
        }
        validate_absolute(&directory)?;
        sources.push(SourceSpec::Directory {
            kind: SourceKind::MailDiagnostics,
            root: directory,
            missing_ok: true,
        });
    }
    if !scope.deployment_receipts_dir.is_empty() {
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
                let root_handle = match open_directory(root) {
                    Ok(directory) => Some(directory),
                    Err(error)
                        if error
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                            && *missing_ok =>
                    {
                        None
                    }

                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("读取备份范围来源失败：{}", root.display()));
                    }
                };
                let present = root_handle.is_some();
                let mut bytes = 0_u64;
                if let Some(root_handle) = root_handle {
                    let source_uid =
                        root_handle.metadata().context("读取备份范围来源属主失败")?.uid();
                    walk(
                        Arc::new(root_handle),
                        root,
                        root,
                        *kind,
                        source_uid,
                        0,
                        plan,
                        &mut artifacts,
                        &mut total,
                        &mut bytes,
                    )?;
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
        let relative = format!("{}/{name}", kind.slug());
        validate_relative(&relative)?;
        let parent_path = path.parent().context("备份范围配置文件缺少父目录")?;
        let parent = Arc::new(open_directory(parent_path)?);
        let component = CString::new(name.as_bytes()).context("备份范围文件名含 NUL")?;
        let file = open_child_file(&parent, &component)
            .with_context(|| format!("打开备份范围文件失败：{}", path.display()))?;
        let artifact = artifact(
            ArtifactInput { kind, relative: &relative, path, parent, name: component, file },
            plan,
            total,
        )?;
        bytes = bytes.checked_add(artifact.bytes).context("备份范围字节数溢出")?;
        push_artifact(artifacts, artifact, plan.max_files)?;
    }
    Ok(bytes)
}

#[expect(
    clippy::too_many_arguments,
    reason = "递归遍历同时携带来源身份、深度与三个累加器，打包成结构体只会把同一组可变借用换个名字"
)]
fn walk(
    directory_handle: Arc<File>,
    root: &Path,
    directory: &Path,
    kind: SourceKind,
    source_uid: u32,
    depth: usize,
    plan: &Plan,
    artifacts: &mut Vec<Artifact>,
    total: &mut u64,
    bytes: &mut u64,
) -> anyhow::Result<()> {
    if depth > MAX_DEPTH {
        anyhow::bail!("备份范围目录层级超过 {MAX_DEPTH}：{}", directory.display());
    }
    let mut entries = read_directory_names(&directory_handle)
        .with_context(|| format!("枚举备份范围目录失败：{}", directory.display()))?;
    entries.sort_unstable();
    let prefix = directory
        .strip_prefix(root)
        .context("备份范围目录不在来源根下")?
        .to_string_lossy()
        .into_owned();
    for entry in entries {
        let os_name = entry;
        let name = match os_name.to_str() {
            Some(name) => name,
            None => anyhow::bail!("备份范围文件名必须是 UTF-8：{}", os_name.to_string_lossy()),
        };
        validate_component(name)?;
        let component = CString::new(os_name.as_bytes()).context("备份范围文件名含 NUL")?;
        let path = directory.join(name);
        let relative = if prefix.is_empty() {
            format!("{}/{name}", kind.slug())
        } else {
            format!("{}/{prefix}/{name}", kind.slug())
        };
        if relative.len() > MAX_RELATIVE_BYTES {
            anyhow::bail!("备份范围相对路径超过 {MAX_RELATIVE_BYTES} 字节：{relative}");
        }
        let file = open_child_file(&directory_handle, &component)
            .with_context(|| format!("打开备份范围文件失败：{}", path.display()))?;
        let metadata = file.metadata().context("读取备份范围文件元数据失败")?;
        if metadata.is_dir() {
            let descriptor = unsafe {
                libc::openat(
                    directory_handle.as_raw_fd(),
                    component.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if descriptor < 0 {
                return Err(std::io::Error::last_os_error())
                    .with_context(|| format!("打开备份范围子目录失败：{}", path.display()));
            }
            let child = unsafe { File::from_raw_fd(descriptor) };
            let child_metadata = child.metadata().context("读取备份范围子目录元数据失败")?;
            if !child_metadata.is_dir()
                || child_metadata.dev() != metadata.dev()
                || child_metadata.ino() != metadata.ino()
            {
                anyhow::bail!("备份范围子目录在打开时被替换：{}", path.display());
            }
            if let Some(problem) = source_entry_problem(Facts::of(&child_metadata), source_uid) {
                anyhow::bail!(
                    "备份范围子目录不符合来源属主/写入策略（{problem}）：{}",
                    path.display()
                );
            }
            walk(
                Arc::new(child),
                root,
                &path,
                kind,
                source_uid,
                depth + 1,
                plan,
                artifacts,
                total,
                bytes,
            )?;
            continue;
        }
        if !metadata.is_file() {
            anyhow::bail!("备份范围不得包含特殊文件：{}", path.display());
        }
        if let Some(problem) = source_entry_problem(Facts::of(&metadata), source_uid) {
            anyhow::bail!("备份范围文件不符合来源属主/权限策略（{problem}）：{}", path.display());
        }
        let artifact = artifact(
            ArtifactInput {
                kind,
                relative: &relative,
                path: &path,
                parent: Arc::clone(&directory_handle),
                name: component,
                file,
            },
            plan,
            total,
        )?;
        *bytes = bytes.checked_add(artifact.bytes).context("备份范围字节数溢出")?;
        push_artifact(artifacts, artifact, plan.max_files)?;
    }
    Ok(())
}

struct ArtifactInput<'a> {
    kind: SourceKind,
    relative: &'a str,
    path: &'a Path,
    parent: Arc<File>,
    name: CString,
    file: File,
}

fn artifact(input: ArtifactInput<'_>, plan: &Plan, total: &mut u64) -> anyhow::Result<Artifact> {
    let ArtifactInput { kind, relative, path, parent, name, file } = input;
    let metadata = file.metadata().context("读取备份范围文件元数据失败")?;
    let parent_metadata = parent.metadata().context("读取备份范围文件父目录元数据失败")?;
    if !metadata.is_file()
        || metadata.uid() != parent_metadata.uid()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        anyhow::bail!(
            "备份范围文件必须与来源目录同属主且为 owner-only 单链接普通文件：{}",
            path.display()
        );
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
        parent,
        name,
        bytes: metadata.len(),
        identity: FileIdentity::of(&metadata),
        modified_seconds: metadata.mtime(),
        modified_nanos: metadata.mtime_nsec(),
    })
}

/// 在收集与打包之间都相对于固定父目录打开，不能被路径祖先或目录项替换重定向。
pub(crate) fn open_verified(artifact: &Artifact) -> anyhow::Result<File> {
    let file = open_child_file(&artifact.parent, &artifact.name)
        .with_context(|| format!("打开备份范围文件失败：{}", artifact.path))?;
    let metadata = file.metadata().context("读取备份范围文件元数据失败")?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.len() != artifact.bytes
        || FileIdentity::of(&metadata) != artifact.identity
        || metadata.mtime() != artifact.modified_seconds
        || metadata.mtime_nsec() != artifact.modified_nanos
    {
        anyhow::bail!("备份范围文件在收集后变化或不再符合权限策略：{}", artifact.path);
    }
    Ok(file)
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

/// 每加入一项前执行计数上限，限制超大目录的收集内存。
fn push_artifact(
    artifacts: &mut Vec<Artifact>,
    artifact: Artifact,
    max_files: u32,
) -> anyhow::Result<()> {
    if artifacts.len() >= max_files as usize {
        anyhow::bail!("备份范围文件数超过 {max_files} 上限");
    }
    artifacts.push(artifact);
    Ok(())
}

/// 收集完成后再次核对计数上限。
pub(crate) fn enforce_file_count(artifacts: &[Artifact], max_files: u32) -> anyhow::Result<()> {
    if artifacts.len() as u64 > u64::from(max_files) {
        anyhow::bail!("备份范围文件数超过 {max_files} 上限");
    }
    Ok(())
}

fn source_entry_problem(facts: Facts, source_uid: u32) -> Option<&'static str> {
    if facts.uid != source_uid {
        return Some("属主与来源目录不一致");
    }
    match facts.kind {
        Kind::Directory if facts.mode & 0o022 != 0 => Some("目录允许 group/other 写入"),
        Kind::File if facts.mode & 0o077 != 0 => Some("文件不是 owner-only"),
        Kind::Directory | Kind::File => None,
        Kind::Other => Some("不是普通文件或目录"),
    }
}

#[cfg(test)]
mod tests {
    use super::source_entry_problem;
    use crate::support::files::{Facts, Kind};

    fn facts(kind: Kind, uid: u32, mode: u32) -> Facts {
        Facts { kind, uid, mode }
    }

    #[test]
    fn accepts_service_owned_entries_when_caller_uid_differs() {
        let service_uid = 990;
        assert_eq!(
            source_entry_problem(facts(Kind::Directory, service_uid, 0o040700), service_uid),
            None
        );
        assert_eq!(
            source_entry_problem(facts(Kind::File, service_uid, 0o100600), service_uid),
            None
        );
    }

    #[test]
    fn rejects_unsafe_nested_owner_and_modes() {
        let source_uid = 990;
        assert_eq!(
            source_entry_problem(facts(Kind::Directory, 991, 0o040700), source_uid),
            Some("属主与来源目录不一致")
        );
        assert_eq!(
            source_entry_problem(facts(Kind::Directory, source_uid, 0o040770), source_uid),
            Some("目录允许 group/other 写入")
        );
        assert_eq!(
            source_entry_problem(facts(Kind::File, 991, 0o100600), source_uid),
            Some("属主与来源目录不一致")
        );
        assert_eq!(
            source_entry_problem(facts(Kind::File, source_uid, 0o100640), source_uid),
            Some("文件不是 owner-only")
        );
    }
}
