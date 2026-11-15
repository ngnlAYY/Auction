//! 旧程序保留、逐个原子替换与回滚记录：安装前把当前运行的程序复制到状态目录，回执式记录
//! 版本区间与摘要；每个程序以同目录临时文件 + rename 替换，失败不留下半写程序。四个程序
//! 整体不是原子的：制品安装中途失败时自动把已替换的程序从回滚记录换回（见 `restore`），
//! 换回也失败时安装目录才可能新旧混合。

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::artifact::Artifact;
use crate::app::progress;
use crate::support::digest::{is_lower_hex, sha256_path};
use crate::support::files;

mod restore;

pub(crate) const FORMAT: &str = "auction-deploy-rollback.v1";
/// 回滚记录目录中的记录文件名；保留程序在同一记录目录的 `bin/` 下。
pub(crate) const RECORD_FILE: &str = "record.json";
const MAX_RECORD_BYTES: u64 = 256 * 1024;
const DIRECTORY: &str = "rollback";

/// 保留的旧程序；只记录名称、摘要与字节数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedBinary {
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
}

/// 一次 deploy 之前安装的程序与版本区间；回退只使用本条记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RollbackRecord {
    pub format: String,
    pub record_id: Uuid,
    pub deploy_receipt_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub install_directory: String,
    pub previous_expected_version: i64,
    pub deployed_expected_version: i64,
    pub previous_source_commit: Option<String>,
    pub binaries: Vec<RetainedBinary>,
}

impl RollbackRecord {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if self.format != FORMAT {
            anyhow::bail!("回滚记录格式不受支持：{}", self.format);
        }
        if self.record_id.is_nil()
            || self.deploy_receipt_id.is_nil()
            || self.record_id != self.deploy_receipt_id
        {
            anyhow::bail!("回滚记录的标识无效");
        }
        if !self.install_directory.starts_with('/')
            || self.install_directory.trim() != self.install_directory
        {
            anyhow::bail!("回滚记录的安装目录必须是绝对路径");
        }
        if self.previous_expected_version < 0 || self.deployed_expected_version < 0 {
            anyhow::bail!("回滚记录的迁移版本无效");
        }
        if let Some(commit) = &self.previous_source_commit
            && !is_lower_hex(commit, 40)
        {
            anyhow::bail!("回滚记录的来源提交无效");
        }
        if self.binaries.len() != super::artifact::REQUIRED_BINARIES.len() {
            anyhow::bail!("回滚记录必须包含四个程序");
        }
        for name in super::artifact::REQUIRED_BINARIES {
            let matches = self.binaries.iter().filter(|binary| binary.name == name).count();
            if matches != 1 {
                anyhow::bail!("回滚记录必须恰好记录一次 {name}");
            }
        }
        for binary in &self.binaries {
            if !is_lower_hex(&binary.sha256, 64) || binary.bytes == 0 {
                anyhow::bail!("回滚记录的程序摘要无效：{}", binary.name);
            }
        }
        Ok(())
    }

    pub(crate) fn binary(&self, name: &str) -> Option<&RetainedBinary> {
        self.binaries.iter().find(|binary| binary.name == name)
    }
}

fn rollback_root(state_dir: &Path) -> PathBuf {
    state_dir.join(DIRECTORY)
}

/// 单条回滚记录的目录：`<state-dir>/rollback/<record_id>/`，保留程序在其 `bin/` 下。
pub(crate) fn record_dir(state_dir: &Path, record_id: Uuid) -> PathBuf {
    rollback_root(state_dir).join(record_id.to_string())
}

/// 把安装目录中当前生效的四个程序复制到回滚记录；任何一步不完整都整体失败。
pub(crate) fn retain(
    state_dir: &Path,
    deploy_receipt_id: Uuid,
    install_dir: &Path,
    previous_expected_version: i64,
    deployed_expected_version: i64,
    previous_source_commit: Option<&str>,
) -> anyhow::Result<RollbackRecord> {
    if deploy_receipt_id.is_nil() {
        anyhow::bail!("回滚记录需要非 nil 的部署回执标识");
    }
    if previous_expected_version < 0 || deployed_expected_version < 0 {
        anyhow::bail!("回滚记录的迁移版本无效");
    }
    let directory = record_dir(state_dir, deploy_receipt_id);
    files::ensure_private_dir(&rollback_root(state_dir)).context("准备回滚记录目录失败")?;
    if directory.exists() {
        anyhow::bail!("回滚记录已存在，拒绝覆盖：{}", directory.display());
    }
    files::ensure_private_dir(&directory).context("创建回滚记录目录失败")?;
    let binaries_dir = directory.join("bin");
    files::ensure_private_dir(&binaries_dir).context("创建回滚程序目录失败")?;
    let mut binaries = Vec::with_capacity(super::artifact::REQUIRED_BINARIES.len());
    for name in super::artifact::REQUIRED_BINARIES {
        let source = install_dir.join(name);
        let metadata = fs::symlink_metadata(&source)
            .with_context(|| format!("读取当前安装程序失败：{}", source.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
            anyhow::bail!("当前安装程序不是可保留的普通文件：{}", source.display());
        }
        let target = binaries_dir.join(name);
        copy_executable(&source, &target)?;
        let (sha256, bytes) = sha256_path(&target)?;
        let (source_sha256, _) = sha256_path(&source)?;
        if sha256 != source_sha256 {
            anyhow::bail!("保留的旧程序摘要与安装目录不一致：{name}");
        }
        binaries.push(RetainedBinary { name: name.to_owned(), sha256, bytes });
    }
    files::sync_directory(&binaries_dir, "安装目录")?;
    let record = RollbackRecord {
        format: FORMAT.to_owned(),
        record_id: deploy_receipt_id,
        deploy_receipt_id,
        created_at: Utc::now(),
        install_directory: install_dir.display().to_string(),
        previous_expected_version,
        deployed_expected_version,
        previous_source_commit: previous_source_commit.map(str::to_owned),
        binaries,
    };
    record.validate()?;
    let bytes = serde_json::to_vec_pretty(&record).context("序列化回滚记录失败")?;
    files::write_private(&directory.join(RECORD_FILE), &bytes)?;
    Ok(record)
}

/// 读取单条回滚记录并复核保留程序仍与记录摘要一致。
pub(crate) fn load(state_dir: &Path, record_id: Uuid) -> anyhow::Result<(RollbackRecord, PathBuf)> {
    if record_id.is_nil() {
        anyhow::bail!("回滚记录标识不能是 nil UUID");
    }
    let directory = record_dir(state_dir, record_id);
    load_from(&directory, record_id)
}

/// 选择最新回滚记录；没有记录时失败，由调用方给出明确指引。
///
/// 分两步：先只读取并校验每个候选的 `record.json`，按 `created_at` 选出最新一条；再只对选中
/// 的记录散列保留程序，较旧记录缺少保留程序不影响选择。`retain` 最后才写 `record.json`，缺少
/// 它的目录是保留中途失败留下的残留，从未成为任何回执的回退目标，跳过并在 stderr 点名；
/// `record.json` 存在但读不出、损坏或无效时点名失败关闭，不静默跳过，否则回退目标可能不是
/// 运维预期的版本。选中记录之后 `rollback` 还会核对安装目录必须是它对应的部署版本。
pub(crate) fn latest(state_dir: &Path) -> anyhow::Result<(RollbackRecord, PathBuf)> {
    let root = rollback_root(state_dir);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("状态目录中没有回滚记录；先执行 auction-cli deploy")
        }
        Err(error) => return Err(error).context("读取回滚记录目录失败"),
    };
    let mut latest: Option<RollbackRecord> = None;
    let mut tied = false;
    for entry in entries {
        let entry = entry.context("读取回滚记录目录项失败")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(record_id) = Uuid::parse_str(name) else { continue };
        let directory = record_dir(state_dir, record_id);
        if is_unfinished(&directory)? {
            progress(format_args!(
                "跳过缺少 {RECORD_FILE} 的未完成回滚记录目录（保留中途失败的残留，可人工核对后\
                 整目录删除）：{}",
                directory.display()
            ));
            continue;
        }
        let candidate = read_record(&directory, record_id).with_context(|| {
            format!(
                "回滚记录无效：{}；用 --receipt-id 指定目标记录，或人工核对该目录",
                directory.display()
            )
        })?;
        match &latest {
            Some(current) if current.created_at > candidate.created_at => {}
            Some(current) if current.created_at == candidate.created_at => tied = true,
            _ => {
                latest = Some(candidate);
                tied = false;
            }
        }
    }
    let record = latest.context("状态目录中没有可用的回滚记录；先执行 auction-cli deploy")?;
    if tied {
        anyhow::bail!("多条回滚记录的创建时间相同，无法确定最新一条；请用 --receipt-id 指定");
    }
    load_from(&record_dir(state_dir, record.record_id), record.record_id)
}

fn load_from(directory: &Path, record_id: Uuid) -> anyhow::Result<(RollbackRecord, PathBuf)> {
    let record = read_record(directory, record_id)?;
    for binary in &record.binaries {
        let path = directory.join("bin").join(&binary.name);
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("读取保留程序失败：{}", path.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            anyhow::bail!("保留程序不是普通文件：{}", path.display());
        }
        let (sha256, bytes) = sha256_path(&path)?;
        if sha256 != binary.sha256 || bytes != binary.bytes {
            anyhow::bail!("保留程序摘要与回滚记录不一致：{}", binary.name);
        }
    }
    Ok((record, directory.to_path_buf()))
}

/// 记录目录是非符号链接目录但没有 `record.json`：保留在写出记录文件之前中止。其他形态交给
/// [`read_record`] 点名失败；`record.json` 无法判断是否存在时同样失败关闭。
fn is_unfinished(directory: &Path) -> anyhow::Result<bool> {
    let metadata = fs::symlink_metadata(directory)
        .with_context(|| format!("读取回滚记录目录失败：{}", directory.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let record = directory.join(RECORD_FILE);
    match fs::symlink_metadata(&record) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).with_context(|| format!("读取回滚记录失败：{}", record.display())),
    }
}

/// 只读取并校验 `record.json`，不散列保留程序。
fn read_record(directory: &Path, record_id: Uuid) -> anyhow::Result<RollbackRecord> {
    let metadata = fs::symlink_metadata(directory)
        .with_context(|| format!("读取回滚记录目录失败：{}", directory.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("回滚记录必须是非符号链接目录：{}", directory.display());
    }
    let bytes = files::read_bounded(&directory.join(RECORD_FILE), MAX_RECORD_BYTES)
        .context("读取回滚记录失败")?;
    let record: RollbackRecord = serde_json::from_slice(&bytes).context("解析回滚记录失败")?;
    record.validate()?;
    if record.record_id != record_id {
        anyhow::bail!("回滚记录标识与目录名不一致");
    }
    Ok(record)
}

/// 制品安装失败；`restored` 为真表示安装目录四个程序已复核为回滚记录中的旧程序。
#[derive(Debug)]
pub(crate) struct InstallArtifactFailure {
    pub(crate) error: anyhow::Error,
    pub(crate) restored: bool,
}

/// 安装制品中的四个程序；临时文件与目标同目录，摘要复核后才 rename 覆盖。
///
/// 不变量：四个程序整体不是原子的。任一程序失败时，本次已经 rename 的程序（含 rename 之后
/// 复核失败的那一个）立即从 `retained` 换回，再按记录复核全部四个程序；这发生在迁移之前，
/// 不启动服务。换回失败时不重试，错误列出各程序的当前摘要，交由人工核对。
pub(crate) fn install_artifact(
    artifact: &Artifact,
    install_dir: &Path,
    owner_uid: u32,
    retained: &RollbackRecord,
    record_directory: &Path,
) -> Result<(), InstallArtifactFailure> {
    let mut replaced = Vec::with_capacity(artifact.binaries.len());
    for binary in &artifact.binaries {
        let target = install_dir.join(binary.name);
        if let Err(failure) =
            install_one(&binary.path, &target, &binary.sha256, binary.bytes, owner_uid)
        {
            if failure.replaced {
                replaced.push(binary.name);
            }
            progress(format_args!(
                "安装 {} 失败，从回滚记录换回本次已替换的程序并复核安装目录",
                binary.name
            ));
            let swap = restore::Swap { retained, record_directory, install_dir, owner_uid };
            return Err(swap.back(failure.error, &replaced));
        }
        replaced.push(binary.name);
    }
    Ok(())
}

/// 安装回滚记录保留的程序；先复核记录摘要，再从记录目录复制。
pub(crate) fn install_retained(
    record: &RollbackRecord,
    record_directory: &Path,
    install_dir: &Path,
    owner_uid: u32,
) -> anyhow::Result<()> {
    for binary in &record.binaries {
        install_one(
            &record_directory.join("bin").join(&binary.name),
            &install_dir.join(&binary.name),
            &binary.sha256,
            binary.bytes,
            owner_uid,
        )
        .map_err(|failure| failure.error)?;
    }
    Ok(())
}

/// 安装目录必须存在、非符号链接，且当前账号有权替换其中的程序。
pub(crate) fn require_install_dir(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("--install-dir 必须是绝对路径");
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("读取安装目录失败：{}", path.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("安装目录必须是非符号链接目录：{}", path.display());
    }
    let euid = files::effective_uid();
    if metadata.uid() != euid && euid != 0 {
        anyhow::bail!("安装目录必须归当前账号或 root 所有：{}", path.display());
    }
    Ok(())
}

/// 单个程序的安装失败；`replaced` 表示失败发生在 rename 之后，目标可能已是新程序。
struct InstallFailure {
    error: anyhow::Error,
    replaced: bool,
}

fn install_one(
    source: &Path,
    target: &Path,
    expected_sha256: &str,
    expected_bytes: u64,
    owner_uid: u32,
) -> Result<(), InstallFailure> {
    let mut temporary = None;
    let mut replaced = false;
    let result = (|| -> anyhow::Result<()> {
        let metadata = fs::symlink_metadata(source)
            .with_context(|| format!("读取待安装程序失败：{}", source.display()))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != expected_bytes
        {
            anyhow::bail!("待安装程序不是符合记录大小的普通文件：{}", source.display());
        }
        let directory = target.parent().context("安装目标缺少父目录")?;
        let name =
            target.file_name().and_then(|name| name.to_str()).context("安装目标缺少文件名")?;
        let staged: &Path =
            temporary.insert(directory.join(format!(".{name}.{}.new", Uuid::new_v4())));
        copy_executable(source, staged)?;
        let (sha256, bytes) = sha256_path(staged)?;
        if sha256 != expected_sha256 || bytes != expected_bytes {
            anyhow::bail!("安装临时文件的摘要与制品不一致：{name}");
        }
        set_owner(staged, owner_uid)?;
        fs::rename(staged, target)
            .with_context(|| format!("原子替换程序失败：{}", target.display()))?;
        replaced = true;
        files::sync_directory(directory, "安装目录")?;
        let (installed_sha256, installed_bytes) = sha256_path(target)?;
        if installed_sha256 != expected_sha256 || installed_bytes != expected_bytes {
            anyhow::bail!("安装后的程序摘要不一致：{name}");
        }
        Ok(())
    })();
    result.map_err(|error| {
        if let Some(temporary) = &temporary {
            let _ = fs::remove_file(temporary);
        }
        InstallFailure { error, replaced }
    })
}

fn copy_executable(source: &Path, target: &Path) -> anyhow::Result<()> {
    let mut input =
        File::open(source).with_context(|| format!("打开源程序失败：{}", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(target)
        .with_context(|| format!("创建目标程序失败：{}", target.display()))?;
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = input.read(&mut buffer).context("读取源程序失败")?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read]).context("写入目标程序失败")?;
    }
    output.set_permissions(fs::Permissions::from_mode(0o755)).context("设置程序权限失败")?;
    output.sync_all().context("同步目标程序失败")?;
    Ok(())
}

/// 程序必须归对应 unit 的服务账号所有；非 root 只能在已经是同一 UID 时跳过。
fn set_owner(path: &Path, owner_uid: u32) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).context("读取临时程序元数据失败")?;
    if metadata.uid() == owner_uid {
        return Ok(());
    }
    if files::effective_uid() != 0 {
        anyhow::bail!(
            "安装程序需要 root 才能改归服务账号（UID {owner_uid}）；请以 root 运行 auction-cli deploy"
        );
    }
    let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .context("安装路径含非法字符")?;
    // SAFETY：路径以 NUL 结尾且只在调用期间使用；-1 表示不修改 group。
    let status = unsafe { libc::chown(path_c.as_ptr(), owner_uid, u32::MAX) };
    if status != 0 {
        return Err(std::io::Error::last_os_error()).context("设置程序属主失败");
    }
    Ok(())
}

/// 回滚记录目录的摘要投影；用于回执与 stdout，不包含程序内容。
pub(crate) fn record_projection(record: &RollbackRecord) -> serde_json::Value {
    serde_json::json!({
        "record_id": record.record_id,
        "install_directory": record.install_directory,
        "previous_expected_version": record.previous_expected_version,
        "deployed_expected_version": record.deployed_expected_version,
        "binaries": record.binaries,
    })
}

#[cfg(test)]
mod tests;
