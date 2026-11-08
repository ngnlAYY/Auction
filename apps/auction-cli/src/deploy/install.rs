//! 旧程序保留、原子安装与回滚记录：安装前把当前运行的程序复制到状态目录，回执式记录
//! 版本区间与摘要；安装以同目录临时文件 + rename 完成，失败不留下半写程序。

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

use super::artifact::{Artifact, sha256_path};
use crate::support::files;

pub(crate) const FORMAT: &str = "auction-deploy-rollback.v1";
const RECORD_FILE: &str = "record.json";
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
            && !super::drill::is_lower_hex(commit, 40)
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
            if !super::drill::is_lower_hex(&binary.sha256, 64) || binary.bytes == 0 {
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

fn record_dir(state_dir: &Path, record_id: Uuid) -> PathBuf {
    rollback_root(state_dir).join(record_id.to_string())
}

/// 把安装目录中当前生效的四个程序复制到回滚记录；任何一步不完整都整体失败。
#[allow(clippy::too_many_arguments)]
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
    sync_directory(&binaries_dir)?;
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
pub(crate) fn latest(state_dir: &Path) -> anyhow::Result<(RollbackRecord, PathBuf)> {
    let root = rollback_root(state_dir);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("状态目录中没有回滚记录；先执行 auction-cli deploy")
        }
        Err(error) => return Err(error).context("读取回滚记录目录失败"),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.context("读取回滚记录目录项失败")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(record_id) = Uuid::parse_str(name) else { continue };
        candidates.push(record_id);
    }
    candidates.sort_unstable();
    let mut latest: Option<(RollbackRecord, PathBuf)> = None;
    for record_id in candidates.into_iter().rev() {
        // 最新记录优先；损坏的较新记录不能被静默跳过，否则回退目标可能不是运维预期的版本。
        let candidate = load_from(&record_dir(state_dir, record_id), record_id)?;
        match &latest {
            Some((current, _)) if current.created_at > candidate.0.created_at => {}
            Some(_) => latest = Some(candidate),
            None => latest = Some(candidate),
        }
    }
    latest.context("状态目录中没有可用的回滚记录；先执行 auction-cli deploy")
}

fn load_from(directory: &Path, record_id: Uuid) -> anyhow::Result<(RollbackRecord, PathBuf)> {
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

/// 安装制品中的四个程序；临时文件与目标同目录，摘要复核后才 rename 覆盖。
pub(crate) fn install_artifact(
    artifact: &Artifact,
    install_dir: &Path,
    owner_uid: u32,
) -> anyhow::Result<()> {
    for binary in &artifact.binaries {
        install_one(
            &binary.path,
            &install_dir.join(binary.name),
            &binary.sha256,
            binary.bytes,
            owner_uid,
        )?;
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
        )?;
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

fn install_one(
    source: &Path,
    target: &Path,
    expected_sha256: &str,
    expected_bytes: u64,
    owner_uid: u32,
) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("读取待安装程序失败：{}", source.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != expected_bytes
    {
        anyhow::bail!("待安装程序不是符合记录大小的普通文件：{}", source.display());
    }
    let directory = target.parent().context("安装目标缺少父目录")?;
    let name = target.file_name().and_then(|name| name.to_str()).context("安装目标缺少文件名")?;
    let temporary = directory.join(format!(".{name}.{}.new", Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        copy_executable(source, &temporary)?;
        let (sha256, bytes) = sha256_path(&temporary)?;
        if sha256 != expected_sha256 || bytes != expected_bytes {
            anyhow::bail!("安装临时文件的摘要与制品不一致：{name}");
        }
        set_owner(&temporary, owner_uid)?;
        fs::rename(&temporary, target)
            .with_context(|| format!("原子替换程序失败：{}", target.display()))?;
        sync_directory(directory)?;
        let (installed_sha256, installed_bytes) = sha256_path(target)?;
        if installed_sha256 != expected_sha256 || installed_bytes != expected_bytes {
            anyhow::bail!("安装后的程序摘要不一致：{name}");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
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

fn sync_directory(directory: &Path) -> anyhow::Result<()> {
    File::open(directory)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("同步安装目录失败：{}", directory.display()))
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
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use super::{FORMAT, RollbackRecord, install_artifact, latest, load, retain};
    use crate::deploy::artifact::{Artifact, ArtifactBinary, sha256_bytes};

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("auction-deploy-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).expect("建立临时目录");
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn install_dir(root: &Path) -> PathBuf {
        let directory = root.join("install");
        fs::create_dir_all(&directory).expect("建立安装目录");
        for name in crate::deploy::artifact::REQUIRED_BINARIES {
            let path = directory.join(name);
            fs::write(&path, format!("old-{name}")).expect("写入旧程序");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("设置权限");
        }
        directory
    }

    fn artifact(root: &Path) -> Artifact {
        let binaries_dir = root.join("artifact/bin");
        fs::create_dir_all(&binaries_dir).expect("建立制品目录");
        let mut binaries = Vec::new();
        for name in crate::deploy::artifact::REQUIRED_BINARIES {
            let path = binaries_dir.join(name);
            fs::write(&path, format!("new-{name}")).expect("写入新程序");
            binaries.push(ArtifactBinary {
                name,
                path,
                sha256: sha256_bytes(format!("new-{name}").as_bytes()),
                bytes: format!("new-{name}").len() as u64,
            });
        }
        Artifact {
            root: root.join("artifact"),
            package: "auction-rust-0.1.0-aaaaaaaaaaaa".to_owned(),
            source_commit: "a".repeat(40),
            source_fingerprint: "b".repeat(64),
            binaries,
            public_payload_sha256: "c".repeat(64),
            admin_payload_sha256: "d".repeat(64),
        }
    }

    #[test]
    fn retain_install_and_load_round_trip() {
        let root = TempRoot::new();
        let install = install_dir(root.path());
        let state = root.path().join("state");
        let receipt_id = uuid::Uuid::new_v4();
        let record = retain(&state, receipt_id, &install, 94, 96, Some(&"e".repeat(40)))
            .expect("保留旧程序");
        assert_eq!(record.format, FORMAT);
        let (loaded, directory) = load(&state, receipt_id).expect("读取回滚记录");
        assert_eq!(loaded, record);
        assert_eq!(latest(&state).expect("最新记录").0, record);

        let artifact = artifact(root.path());
        install_artifact(&artifact, &install, crate::support::files::effective_uid())
            .expect("安装制品程序");
        for binary in &artifact.binaries {
            let path = install.join(binary.name);
            assert_eq!(
                fs::read(&path).expect("读取安装程序"),
                format!("new-{}", binary.name).into_bytes()
            );
        }
        // 回退复制保留的旧程序后必须与记录摘要一致。
        super::install_retained(
            &record,
            &directory,
            &install,
            crate::support::files::effective_uid(),
        )
        .expect("恢复旧程序");
        for binary in &record.binaries {
            let (sha256, _) =
                crate::deploy::artifact::sha256_path(&install.join(&binary.name)).expect("摘要");
            assert_eq!(sha256, binary.sha256);
        }
    }

    #[test]
    fn tampered_retained_binary_is_rejected() {
        let root = TempRoot::new();
        let install = install_dir(root.path());
        let state = root.path().join("state");
        let receipt_id = uuid::Uuid::new_v4();
        retain(&state, receipt_id, &install, 94, 96, None).expect("保留旧程序");
        let directory = state.join("rollback").join(receipt_id.to_string());
        fs::write(directory.join("bin/auction-cli"), b"tampered").expect("改写保留程序");
        assert!(load(&state, receipt_id).is_err());
    }

    /// `rollback` 按 `RollbackRecord::deploy_receipt_id` 读回部署回执；该标识必须是
    /// 保留旧程序时已分配、发布回执时实际使用的同一个标识。
    #[cfg(feature = "postgres")]
    #[test]
    fn deploy_receipt_is_readable_from_retained_record_id() {
        use crate::deployment::receipt::{
            Action, DeployRecord, Outcome, ReceiptContext, read as read_receipt, record_deploy,
        };

        let root = TempRoot::new();
        let install = install_dir(root.path());
        let state = root.path().join("state");
        let receipt_id = uuid::Uuid::new_v4();
        let record = retain(&state, receipt_id, &install, 94, 96, None).expect("保留旧程序");
        let context = ReceiptContext::resolve(Some("backup-1"), Some("operator")).expect("上下文");
        record_deploy(
            &state,
            record.deploy_receipt_id,
            Action::Deploy,
            Outcome::Succeeded,
            chrono::Utc::now(),
            &context,
            None,
            Vec::new(),
            DeployRecord {
                artifact_directory: "/srv/artifact".to_owned(),
                package: "auction-rust-0.1.0-aaaaaaaaaaaa".to_owned(),
                source_commit: "a".repeat(40),
                source_fingerprint: "b".repeat(64),
                cli_sha256: "c".repeat(64),
                install_directory: install.display().to_string(),
                public_payload_sha256: "d".repeat(64),
                admin_payload_sha256: "e".repeat(64),
                migration_drill_id: None,
                backup_archive: None,
                backup_manifest: None,
                rollback_record_id: Some(record.record_id),
            },
            None,
        )
        .expect("写入部署回执");

        let published =
            read_receipt(&state, record.deploy_receipt_id).expect("按回滚记录读回部署回执");
        assert_eq!(published.receipt_id, record.record_id);
        assert_eq!(published.action, Action::Deploy);
        assert_eq!(published.outcome, Outcome::Succeeded);
        assert_eq!(published.deploy.expect("部署证据").rollback_record_id, Some(record.record_id));
    }

    #[test]
    fn failed_deploy_receipt_is_readable_with_installed_artifacts() {
        use crate::deployment::receipt::{
            Action, DeployRecord, Outcome, ReceiptContext, read as read_receipt, record_deploy,
        };

        let root = TempRoot::new();
        let install = install_dir(root.path());
        let state = root.path().join("state");
        let artifact = artifact(root.path());
        let record_id = uuid::Uuid::new_v4();
        let record = retain(&state, record_id, &install, 94, 96, None).expect("保留旧程序");
        let artifacts = crate::deploy::installed_artifacts(&artifact).expect("读取静态制品回执");
        let context = ReceiptContext::resolve(None, Some("operator")).expect("上下文");
        record_deploy(
            &state,
            record.deploy_receipt_id,
            Action::Deploy,
            Outcome::Failed,
            chrono::Utc::now(),
            &context,
            None,
            artifacts,
            DeployRecord {
                artifact_directory: artifact.root.display().to_string(),
                package: artifact.package.clone(),
                source_commit: artifact.source_commit.clone(),
                source_fingerprint: artifact.source_fingerprint.clone(),
                cli_sha256: artifact.cli_sha256().to_owned(),
                install_directory: install.display().to_string(),
                public_payload_sha256: artifact.public_payload_sha256.clone(),
                admin_payload_sha256: artifact.admin_payload_sha256.clone(),
                migration_drill_id: None,
                backup_archive: None,
                backup_manifest: None,
                rollback_record_id: Some(record.record_id),
            },
            Some("startup failed".to_owned()),
        )
        .expect("写入失败部署回执");

        let published =
            read_receipt(&state, record.deploy_receipt_id).expect("回滚记录仍能读回失败回执");
        assert_eq!(published.outcome, Outcome::Failed);
        assert_eq!(published.artifacts.len(), 3);
        assert_eq!(published.deploy.expect("部署证据").cli_sha256, artifact.cli_sha256());
    }

    #[test]
    fn record_rejects_inconsistent_versions() {
        let root = TempRoot::new();
        let install = install_dir(root.path());
        let mut record = RollbackRecord {
            format: FORMAT.to_owned(),
            record_id: uuid::Uuid::new_v4(),
            deploy_receipt_id: uuid::Uuid::nil(),
            created_at: chrono::Utc::now(),
            install_directory: install.display().to_string(),
            previous_expected_version: 94,
            deployed_expected_version: 96,
            previous_source_commit: None,
            binaries: Vec::new(),
        };
        record.deploy_receipt_id = record.record_id;
        assert!(record.validate().is_err());
    }
}
