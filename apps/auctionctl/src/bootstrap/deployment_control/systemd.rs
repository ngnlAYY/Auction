//! systemd unit 身份、状态读取和单角色 mutation 边界；参数直接传给已规范化的
//! `systemctl`，不经过 shell，也不决定跨角色启动顺序。

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::Context;

const MAX_SYSTEMCTL_DIAGNOSTIC_BYTES: usize = 2_048;

pub(super) const START_ORDER: [ManagedRole; 3] =
    [ManagedRole::Worker, ManagedRole::Admin, ManagedRole::Public];
const STOP_ORDER: [ManagedRole; 3] = [ManagedRole::Public, ManagedRole::Admin, ManagedRole::Worker];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ManagedRole {
    Worker,
    Admin,
    Public,
}

impl ManagedRole {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Admin => "admin",
            Self::Public => "public",
        }
    }

    pub(super) const fn start_code(self) -> &'static str {
        match self {
            Self::Worker => "worker-start",
            Self::Admin => "admin-start",
            Self::Public => "public-start",
        }
    }

    pub(super) const fn readiness_code(self) -> &'static str {
        match self {
            Self::Worker => "worker-readiness",
            Self::Admin => "admin-readiness",
            Self::Public => "public-readiness",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct UnitSnapshot {
    pub(super) load_state: String,
    pub(super) active_state: String,
    pub(super) sub_state: String,
    pub(super) main_pid: u32,
    pub(super) invocation_id: String,
    pub(super) started_monotonic_micros: u64,
    pub(super) environment: String,
    pub(super) environment_files: String,
    pub(super) unset_environment: String,
    pub(super) pass_environment: String,
    pub(super) pam_name: String,
}

impl UnitSnapshot {
    pub(super) fn running(&self) -> bool {
        self.load_state == "loaded" && self.active_state == "active" && self.main_pid > 0
    }

    pub(super) fn stopped(&self) -> bool {
        self.safely_stopped_for_migration()
    }

    /// 独立迁移只接受 systemd 明确收敛后的停止状态。
    /// `deactivating`、`maintenance` 和未来新增的未知状态即使暂时没有 MainPID，也不能放行
    /// schema 变更；否则会在 ExecStopPost 或其他 unit 状态转换尚未结束时绕过部署锁语义。
    pub(super) fn safely_stopped_for_migration(&self) -> bool {
        matches!(self.load_state.as_str(), "loaded" | "not-found")
            && self.main_pid == 0
            && matches!(self.active_state.as_str(), "inactive" | "failed")
    }
}

pub(super) struct SystemdClient {
    executable: PathBuf,
    units: BTreeMap<ManagedRole, String>,
}

impl SystemdClient {
    pub(super) fn new(
        executable: PathBuf,
        worker_unit: String,
        admin_unit: String,
        public_unit: String,
    ) -> anyhow::Result<Self> {
        let executable = validate_systemctl(&executable)?;
        let mut units = BTreeMap::new();
        for (role, unit) in [
            (ManagedRole::Worker, worker_unit),
            (ManagedRole::Admin, admin_unit),
            (ManagedRole::Public, public_unit),
        ] {
            validate_unit_name(&unit)?;
            if units.values().any(|existing| existing == &unit) {
                anyhow::bail!("三个部署角色必须使用不同的 systemd unit")
            }
            units.insert(role, unit);
        }
        Ok(Self { executable, units })
    }

    pub(super) fn unit(&self, role: ManagedRole) -> &str {
        self.units.get(&role).map(String::as_str).unwrap_or("")
    }

    pub(super) fn snapshots(&self) -> anyhow::Result<BTreeMap<ManagedRole, UnitSnapshot>> {
        START_ORDER
            .into_iter()
            .map(|role| self.snapshot(role).map(|snapshot| (role, snapshot)))
            .collect()
    }

    pub(super) fn snapshot(&self, role: ManagedRole) -> anyhow::Result<UnitSnapshot> {
        let unit = self.unit(role);
        let output = self.output(
            "读取 systemd unit 状态",
            [
                OsStr::new("show"),
                OsStr::new("--property=LoadState"),
                OsStr::new("--property=ActiveState"),
                OsStr::new("--property=SubState"),
                OsStr::new("--property=MainPID"),
                OsStr::new("--property=InvocationID"),
                OsStr::new("--property=ExecMainStartTimestampMonotonic"),
                OsStr::new("--property=Environment"),
                OsStr::new("--property=EnvironmentFiles"),
                OsStr::new("--property=UnsetEnvironment"),
                OsStr::new("--property=PassEnvironment"),
                OsStr::new("--property=PAMName"),
                OsStr::new(unit),
            ],
        )?;
        parse_snapshot(&output.stdout).with_context(|| format!("解析 {unit} 状态失败"))
    }

    pub(super) fn start_unit(&self, role: ManagedRole) -> anyhow::Result<()> {
        let unit = self.unit(role);
        self.output("启动 systemd unit", [OsStr::new("start"), OsStr::new(unit)])?;
        println!("role={} unit={} action=start-requested", role.name(), unit);
        Ok(())
    }

    pub(super) fn stop_unit(&self, role: ManagedRole) -> anyhow::Result<()> {
        let unit = self.unit(role);
        let before = self.snapshot(role)?;
        if before.stopped() {
            println!("role={} unit={} action=already-stopped", role.name(), unit);
            return Ok(());
        }
        if before.load_state != "loaded" {
            anyhow::bail!("{} unit 未装载：{}", role.name(), unit)
        }
        self.output("停止 systemd unit", [OsStr::new("stop"), OsStr::new(unit)])?;
        let after = self.snapshot(role)?;
        if !after.stopped() {
            anyhow::bail!(
                "{} unit 停止后仍处于 {}/{}，TimeoutStopSec 由 systemd unit 管理",
                role.name(),
                after.active_state,
                after.sub_state,
            )
        }
        println!("role={} unit={} action=stopped", role.name(), unit);
        Ok(())
    }

    pub(super) fn stop_all(&self) -> anyhow::Result<()> {
        let mut failures = Vec::new();
        for role in STOP_ORDER {
            if let Err(error) = self.stop_unit(role) {
                failures.push(format!("{}: {error:#}", role.name()));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("停止部署不完整：{}", failures.join(" | "))
        }
    }

    fn output<I, S>(&self, label: &str, arguments: I) -> anyhow::Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new(&self.executable)
            .arg("--no-pager")
            .arg("--no-ask-password")
            .args(arguments)
            .output()
            .with_context(|| format!("{label}：无法执行 {}", self.executable.display()))?;
        if output.status.success() {
            return Ok(output);
        }
        let diagnostic =
            single_line(&String::from_utf8_lossy(&output.stderr), MAX_SYSTEMCTL_DIAGNOSTIC_BYTES);
        anyhow::bail!(
            "{label}失败（exit={}）：{}",
            output.status.code().map_or_else(|| "signal".to_owned(), |code| code.to_string()),
            if diagnostic.is_empty() { "systemctl 未提供诊断" } else { &diagnostic },
        )
    }
}

fn validate_systemctl(path: &Path) -> anyhow::Result<PathBuf> {
    if !path.is_absolute() {
        anyhow::bail!("systemctl 路径必须是绝对路径")
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("规范化 systemctl 路径失败：{}", path.display()))?;
    let metadata = fs::metadata(&canonical).context("读取 systemctl 元数据失败")?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        anyhow::bail!("systemctl 必须解析到可执行普通文件")
    }
    Ok(canonical)
}

fn validate_unit_name(unit: &str) -> anyhow::Result<()> {
    if unit.is_empty()
        || unit.len() > 128
        || unit.starts_with('-')
        || !unit.ends_with(".service")
        || !unit.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@' | b':')
        })
    {
        anyhow::bail!("systemd unit 名称无效：{unit}")
    }
    Ok(())
}

fn parse_snapshot(bytes: &[u8]) -> anyhow::Result<UnitSnapshot> {
    if bytes.len() > 64 * 1024 {
        anyhow::bail!("systemctl show 输出超过 64 KiB 上限")
    }
    let text = std::str::from_utf8(bytes).context("systemctl show 输出不是 UTF-8")?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let (key, value) =
            line.split_once('=').ok_or_else(|| anyhow::anyhow!("systemctl show 输出行缺少等号"))?;
        if values.insert(key, value).is_some() {
            anyhow::bail!("systemctl show 输出包含重复属性")
        }
    }
    let required = |key: &str| {
        values.get(key).copied().ok_or_else(|| anyhow::anyhow!("systemctl show 缺少 {key}"))
    };
    let main_pid = required("MainPID")?.parse::<u32>().context("MainPID 无效")?;
    let started_monotonic_micros = required("ExecMainStartTimestampMonotonic")?
        .parse::<u64>()
        .context("ExecMainStartTimestampMonotonic 无效")?;
    Ok(UnitSnapshot {
        load_state: required("LoadState")?.to_owned(),
        active_state: required("ActiveState")?.to_owned(),
        sub_state: required("SubState")?.to_owned(),
        main_pid,
        invocation_id: required("InvocationID")?.to_owned(),
        started_monotonic_micros,
        environment: required("Environment")?.to_owned(),
        // systemctl 对空的 a(sb) 数组不输出 EnvironmentFiles 行（即使使用 --all）；
        // 只有此复合属性允许缺行表示空，其他身份和环境属性仍须明确存在。
        environment_files: values.get("EnvironmentFiles").copied().unwrap_or("").to_owned(),
        unset_environment: required("UnsetEnvironment")?.to_owned(),
        pass_environment: required("PassEnvironment")?.to_owned(),
        pam_name: required("PAMName")?.to_owned(),
    })
}

pub(super) fn single_line(value: &str, limit: usize) -> String {
    let mut output = String::with_capacity(value.len().min(limit));
    for character in value.chars() {
        if output.len() >= limit {
            break;
        }
        output.push(if character.is_control() { ' ' } else { character });
    }
    output.trim().to_owned()
}
