//! 回滚记录：deploy/rollback 停机后、安装前把将被替换的程序复制到
//! `<state-dir>/rollback/<id>/bin/`，最后才写 `record.json`。
//!
//! v2 分开记录三件事：保留的三角色程序身份（含内嵌 schema，未知为 null）、保留的 auction-cli
//! 身份（只有 deploy 保留 CLI），以及创建记录的操作开始前观测到的数据库版本。身份由调用方按
//! 保留副本的实际摘要判定，不能从其他版本推测。v1 记录只兼容读取：按其原口径（部署前的数据库
//! 版本即旧程序版本）换算为内存形态，文件不改写。
//!
//! 记录目录、`record.json` 与保留程序都相对持锁时固定的部署状态目录描述符新建与读取，并核对
//! 属主与权限（目录 owner-only，`record.json` 0600，保留程序禁止 group/other 写入）；读取记录、
//! 复核摘要与安装之间路径被改名或替换，也不会换成另一份记录或另一组程序。

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::artifact::REQUIRED_BINARIES;
use super::trust::PROGRAM_MODE;
use crate::app::progress;
use crate::receipt::{CliIdentity, RoleIdentity};
use crate::support::digest::{is_lower_hex, sha256_path, sha256_reader};
use crate::support::files::PrivateDirectory;

pub(crate) const FORMAT: &str = "auction-deploy-rollback.v2";
/// 只读兼容的旧格式：四个程序整体保留，版本字段记录部署前后的数据库版本。
pub(crate) const LEGACY_FORMAT: &str = "auction-deploy-rollback.v1";
/// 回滚记录目录中的记录文件名；保留程序在同一记录目录的 `bin/` 下。
pub(crate) const RECORD_FILE: &str = "record.json";
/// rollback 只替换这三个运行角色，安装目录中的 auction-cli 保持不变。
pub(crate) const ROLE_BINARIES: [&str; 3] =
    ["auction-worker", "auction-admin-api", "auction-public-api"];
const CLI_BINARY: &str = "auction-cli";
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

/// 一次 deploy/rollback 替换前保留的程序与版本事实；回退只使用本条记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RollbackRecord {
    pub format: String,
    pub record_id: Uuid,
    pub deploy_receipt_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub install_directory: String,
    /// 创建本记录的操作开始前观测到的数据库迁移版本。
    pub observed_database_version: i64,
    /// 创建本记录的操作所安装的角色程序内嵌 schema；回退目标未知时为 null。
    pub installed_role_schema_version: Option<i64>,
    /// 保留的三角色程序身份；`schema_version` 为 null 表示无法与已知制品绑定。
    pub retained_roles: RoleIdentity,
    /// 保留的 auction-cli；只有 deploy 的记录保留它，供安装中途失败时换回。
    pub retained_cli: Option<CliIdentity>,
    pub binaries: Vec<RetainedBinary>,
}

/// v1 的原始字段；只用于兼容读取。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRollbackRecord {
    format: String,
    record_id: Uuid,
    deploy_receipt_id: Uuid,
    created_at: DateTime<Utc>,
    install_directory: String,
    previous_expected_version: i64,
    deployed_expected_version: i64,
    previous_source_commit: Option<String>,
    binaries: Vec<RetainedBinary>,
}

impl LegacyRollbackRecord {
    /// v1 的 `previous_expected_version` 是部署前的数据库版本：旧口径把它当作旧程序版本，这里
    /// 只把它作为观测值，角色 schema 记为未知（回退门禁按观测值假设并在报告中标明）。v1 保留的
    /// 来源提交取自最近一条成功回执，可能属于另一组件，同样记为未知。
    fn into_record(self) -> anyhow::Result<RollbackRecord> {
        if self.format != LEGACY_FORMAT {
            anyhow::bail!("回滚记录格式不受支持：{}", self.format);
        }
        if self.previous_expected_version < 0 || self.deployed_expected_version < 0 {
            anyhow::bail!("回滚记录的迁移版本无效");
        }
        if self.previous_source_commit.as_deref().is_some_and(|commit| !is_lower_hex(commit, 40)) {
            anyhow::bail!("回滚记录的来源提交无效");
        }
        let cli = self
            .binaries
            .iter()
            .find(|binary| binary.name == CLI_BINARY)
            .map(|binary| CliIdentity::digest_only(&binary.sha256))
            .context("v1 回滚记录必须包含 auction-cli")?;
        let record = RollbackRecord {
            format: LEGACY_FORMAT.to_owned(),
            record_id: self.record_id,
            deploy_receipt_id: self.deploy_receipt_id,
            created_at: self.created_at,
            install_directory: self.install_directory,
            observed_database_version: self.previous_expected_version,
            installed_role_schema_version: Some(self.deployed_expected_version),
            retained_roles: RoleIdentity::default(),
            retained_cli: Some(cli),
            binaries: self.binaries,
        };
        record.validate()?;
        Ok(record)
    }
}

impl RollbackRecord {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if self.format != FORMAT && self.format != LEGACY_FORMAT {
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
        if self.observed_database_version < 0
            || self.installed_role_schema_version.is_some_and(|version| version < 0)
        {
            anyhow::bail!("回滚记录的迁移版本无效");
        }
        self.retained_roles.validate().context("回滚记录的角色身份无效")?;
        if let Some(cli) = &self.retained_cli {
            cli.validate().context("回滚记录的 auction-cli 身份无效")?;
        }
        for name in ROLE_BINARIES {
            if self.binaries.iter().filter(|binary| binary.name == name).count() != 1 {
                anyhow::bail!("回滚记录必须恰好记录一次 {name}");
            }
        }
        let cli = self.binaries.iter().filter(|binary| binary.name == CLI_BINARY).count();
        match (&self.retained_cli, cli) {
            (Some(identity), 1)
                if self.binary(CLI_BINARY).map(|binary| &binary.sha256)
                    == Some(&identity.sha256) => {}
            (None, 0) => {}
            _ => anyhow::bail!("回滚记录的 auction-cli 副本与身份不一致"),
        }
        if self.binaries.len() != ROLE_BINARIES.len() + cli {
            anyhow::bail!("回滚记录包含未知程序");
        }
        if self.format == LEGACY_FORMAT && self.binaries.len() != REQUIRED_BINARIES.len() {
            anyhow::bail!("v1 回滚记录必须包含四个程序");
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

    /// 回退目标角色的内嵌 schema 与来源：已记录时用记录值；未知（v1 记录或无法与已知制品绑定）
    /// 时沿用旧口径，以创建记录时观测到的数据库版本作为假设。
    pub(crate) fn target_schema(&self) -> (i64, &'static str) {
        match self.retained_roles.schema_version {
            Some(version) => (version, "recorded"),
            None => (self.observed_database_version, "assumed_observed_database_version"),
        }
    }
}

/// 单条回滚记录的目录：`<state-dir>/rollback/<record_id>/`，保留程序在其 `bin/` 下。只用于输出
/// 与提示；读写一律经部署状态目录的描述符。
pub(crate) fn record_dir(state_dir: &Path, record_id: Uuid) -> PathBuf {
    state_dir.join(DIRECTORY).join(record_id.to_string())
}

/// 以描述符固定的单条回滚记录目录与其 `bin/`：记录与保留程序只经它们读取，读取记录、复核摘要
/// 与安装之间路径被改名或替换，也不会换成另一份记录或另一组程序。
pub(crate) struct RecordDirectory {
    directory: PrivateDirectory,
    binaries: PrivateDirectory,
}

impl RecordDirectory {
    pub(crate) fn path(&self) -> &Path {
        &self.directory.path
    }

    /// 打开保留程序：当前账号所有、单链接、禁止 group/other 写入的普通文件。
    pub(crate) fn open_program(&self, name: &str) -> anyhow::Result<File> {
        self.binaries.open_owned(name).context("打开保留程序失败")
    }
}

/// 一次保留的输入；`programs` 是本次将被替换的程序（deploy 四个，rollback 只有三个角色）。
pub(crate) struct RetainRequest<'a> {
    pub(crate) deploy_receipt_id: Uuid,
    pub(crate) install_dir: &'a Path,
    pub(crate) programs: &'a [&'static str],
    pub(crate) observed_database_version: i64,
    pub(crate) installed_role_schema_version: Option<i64>,
}

/// 按保留副本的实际摘要判定的身份；`cli` 只在摘要与副本一致时采用其来源。
pub(crate) struct RetainedIdentity {
    pub(crate) roles: RoleIdentity,
    pub(crate) cli: Option<CliIdentity>,
}

/// 把安装目录中将被替换的程序复制到回滚记录，再由 `identify` 按副本摘要给出身份，最后写
/// `record.json`；任何一步不完整都整体失败，缺少 `record.json` 的目录不会成为回退目标。
/// `state` 是持锁时固定的部署状态目录，记录目录与保留副本都相对它的描述符新建。
pub(crate) fn retain(
    state: &PrivateDirectory,
    request: &RetainRequest<'_>,
    identify: impl FnOnce(&[RetainedBinary]) -> anyhow::Result<RetainedIdentity>,
) -> anyhow::Result<(RollbackRecord, RecordDirectory)> {
    if request.deploy_receipt_id.is_nil() {
        anyhow::bail!("回滚记录需要非 nil 的部署回执标识");
    }
    let root = state
        .open_child(DIRECTORY, true)
        .context("准备回滚记录目录失败")?
        .context("准备回滚记录目录失败")?;
    // 新建即独占：同一标识的记录已存在时拒绝，不覆盖任何保留副本。
    let directory = root
        .create_child(&request.deploy_receipt_id.to_string())
        .context("创建回滚记录目录失败")?;
    let binaries_dir = directory
        .open_child("bin", true)
        .context("创建回滚程序目录失败")?
        .context("创建回滚程序目录失败")?;
    let mut binaries = Vec::with_capacity(request.programs.len());
    for name in request.programs {
        let source = request.install_dir.join(name);
        let metadata = fs::symlink_metadata(&source)
            .with_context(|| format!("读取当前安装程序失败：{}", source.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
            anyhow::bail!("当前安装程序不是可保留的普通文件：{}", source.display());
        }
        copy_executable(&source, &binaries_dir, name)?;
        let (sha256, bytes) = sha256_reader(binaries_dir.open_owned(name)?)
            .with_context(|| format!("读取保留程序失败：{name}"))?;
        let (source_sha256, _) = sha256_path(&source)?;
        if sha256 != source_sha256 {
            anyhow::bail!("保留的旧程序摘要与安装目录不一致：{name}");
        }
        binaries.push(RetainedBinary { name: (*name).to_owned(), sha256, bytes });
    }
    binaries_dir.sync()?;
    let identity = identify(&binaries)?;
    let retained_cli =
        binaries.iter().find(|binary| binary.name == CLI_BINARY).map(|binary| match identity.cli {
            Some(cli) if cli.sha256 == binary.sha256 => cli,
            _ => CliIdentity::digest_only(&binary.sha256),
        });
    let record = RollbackRecord {
        format: FORMAT.to_owned(),
        record_id: request.deploy_receipt_id,
        deploy_receipt_id: request.deploy_receipt_id,
        created_at: Utc::now(),
        install_directory: request.install_dir.display().to_string(),
        observed_database_version: request.observed_database_version,
        installed_role_schema_version: request.installed_role_schema_version,
        retained_roles: identity.roles,
        retained_cli,
        binaries,
    };
    record.validate()?;
    let bytes = serde_json::to_vec_pretty(&record).context("序列化回滚记录失败")?;
    directory.write(RECORD_FILE, &bytes, false).context("写入回滚记录失败")?;
    Ok((record, RecordDirectory { directory, binaries: binaries_dir }))
}

/// 读取单条回滚记录并复核保留程序仍与记录摘要一致。
pub(crate) fn load(
    state: &PrivateDirectory,
    record_id: Uuid,
) -> anyhow::Result<(RollbackRecord, RecordDirectory)> {
    if record_id.is_nil() {
        anyhow::bail!("回滚记录标识不能是 nil UUID");
    }
    let missing = || format!("回滚记录不存在：{}", record_dir(&state.path, record_id).display());
    let root = state.open_child(DIRECTORY, false)?.with_context(missing)?;
    let directory = root.open_child(&record_id.to_string(), false)?.with_context(missing)?;
    load_from(directory, record_id)
}

/// 选择最新回滚记录；没有记录时失败，由调用方给出明确指引。
///
/// 分两步：先只读取并校验每个候选的 `record.json`，按 `created_at` 选出最新一条；再只对选中
/// 的记录散列保留程序，较旧记录缺少保留程序不影响选择。`retain` 最后才写 `record.json`，缺少
/// 它的目录是保留中途失败留下的残留，从未成为任何回执的回退目标，跳过并在 stderr 点名；
/// `record.json` 存在但读不出、损坏或无效时点名失败关闭，不静默跳过，否则回退目标可能不是
/// 运维预期的版本。选中记录之后 `rollback` 还会核对安装目录必须是它对应的部署版本。
pub(crate) fn latest(
    state: &PrivateDirectory,
) -> anyhow::Result<(RollbackRecord, RecordDirectory)> {
    let root = state
        .open_child(DIRECTORY, false)
        .context("读取回滚记录目录失败")?
        .context("状态目录中没有回滚记录；先执行 auction-cli deploy")?;
    let mut latest: Option<RollbackRecord> = None;
    let mut tied = false;
    for name in root.names()? {
        let Ok(record_id) = Uuid::parse_str(&name) else { continue };
        let invalid = || {
            format!(
                "回滚记录无效：{}；用 --receipt-id 指定目标记录，或人工核对该目录",
                root.path.join(&name).display()
            )
        };
        // 记录目录必须是当前账号的 owner-only 真实目录；符号链接、其他账号的目录与普通文件一律
        // 点名失败，不当作残留跳过。
        let directory =
            root.open_child(&name, false).with_context(invalid)?.with_context(invalid)?;
        if !directory.contains(RECORD_FILE).with_context(invalid)? {
            progress(format_args!(
                "跳过缺少 {RECORD_FILE} 的未完成回滚记录目录（保留中途失败的残留，可人工核对后\
                 整目录删除）：{}",
                directory.path.display()
            ));
            continue;
        }
        let candidate = read_record(&directory, record_id).with_context(invalid)?;
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
    let directory = root
        .open_child(&record.record_id.to_string(), false)?
        .context("选中的回滚记录目录已不存在")?;
    load_from(directory, record.record_id)
}

/// 读取记录并经同一组描述符复核每个保留程序的摘要与字节数。
fn load_from(
    directory: PrivateDirectory,
    record_id: Uuid,
) -> anyhow::Result<(RollbackRecord, RecordDirectory)> {
    let record = read_record(&directory, record_id)?;
    let binaries = directory
        .open_child("bin", false)
        .context("读取回滚程序目录失败")?
        .with_context(|| format!("回滚记录缺少 bin/：{}", directory.path.display()))?;
    let record_directory = RecordDirectory { directory, binaries };
    for binary in &record.binaries {
        let file = record_directory
            .open_program(&binary.name)
            .with_context(|| format!("读取保留程序失败：{}", binary.name))?;
        let (sha256, bytes) =
            sha256_reader(file).with_context(|| format!("读取保留程序失败：{}", binary.name))?;
        if sha256 != binary.sha256 || bytes != binary.bytes {
            anyhow::bail!("保留程序摘要与回滚记录不一致：{}", binary.name);
        }
    }
    Ok((record, record_directory))
}

/// 只读取并校验 `record.json`（当前账号所有的 owner-only 单链接普通文件），不散列保留程序；按
/// `format` 分派 v2 或 v1 兼容读取。
fn read_record(directory: &PrivateDirectory, record_id: Uuid) -> anyhow::Result<RollbackRecord> {
    let bytes = directory.read(RECORD_FILE).context("读取回滚记录失败")?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        anyhow::bail!("回滚记录超过 {MAX_RECORD_BYTES} 字节上限");
    }
    let record = parse_record(&bytes)?;
    if record.record_id != record_id {
        anyhow::bail!("回滚记录标识与目录名不一致");
    }
    Ok(record)
}

fn parse_record(bytes: &[u8]) -> anyhow::Result<RollbackRecord> {
    let value: serde_json::Value = serde_json::from_slice(bytes).context("解析回滚记录失败")?;
    match value.get("format").and_then(serde_json::Value::as_str) {
        Some(FORMAT) => {
            let record: RollbackRecord =
                serde_json::from_value(value).context("解析回滚记录失败")?;
            record.validate()?;
            Ok(record)
        }
        Some(LEGACY_FORMAT) => serde_json::from_value::<LegacyRollbackRecord>(value)
            .context("解析 v1 回滚记录失败")?
            .into_record(),
        Some(other) => anyhow::bail!("回滚记录格式不受支持：{other}"),
        None => anyhow::bail!("回滚记录缺少 format"),
    }
}

/// 以 O_NOFOLLOW 打开源程序，在固定的目录中复制为 create_new 的 0755 新文件并同步；不跟随任何
/// 符号链接。
fn copy_executable(source: &Path, directory: &PrivateDirectory, name: &str) -> anyhow::Result<()> {
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(source)
        .with_context(|| format!("打开源程序失败：{}", source.display()))?;
    let mut output = directory.create_file(name, PROGRAM_MODE).context("创建保留程序失败")?;
    copy_contents(&mut input, &mut output)?;
    output.set_permissions(fs::Permissions::from_mode(PROGRAM_MODE)).context("设置程序权限失败")?;
    output.sync_all().context("同步目标程序失败")?;
    Ok(())
}

/// 流式复制已打开的两个文件。
pub(super) fn copy_contents(input: &mut File, output: &mut File) -> anyhow::Result<()> {
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = input.read(&mut buffer).context("读取源程序失败")?;
        if read == 0 {
            return Ok(());
        }
        output.write_all(&buffer[..read]).context("写入目标程序失败")?;
    }
}

/// 回滚记录的摘要投影；用于回执与 stdout，不包含程序内容。
pub(crate) fn record_projection(record: &RollbackRecord) -> serde_json::Value {
    serde_json::json!({
        "record_id": record.record_id,
        "format": record.format,
        "install_directory": record.install_directory,
        "observed_database_version": record.observed_database_version,
        "installed_role_schema_version": record.installed_role_schema_version,
        "retained_roles": record.retained_roles,
        "retained_cli": record.retained_cli,
        "binaries": record.binaries,
    })
}
