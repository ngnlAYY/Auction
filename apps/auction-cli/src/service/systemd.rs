//! systemctl 的有界异步调用；只读取 unit 属性或请求启停，不解析或回显 unit 环境中的其他值。

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::Context;
use serde::Serialize;
use tokio::process::Command;

use crate::cli::UnitArgs;

const SHOW_TIMEOUT: Duration = Duration::from_secs(10);
const START_TIMEOUT: Duration = Duration::from_secs(90);
/// systemd unit 自己的有限停止预算之外，给客户端回执额外留十秒。
const STOP_RECEIPT_MARGIN: Duration = Duration::from_secs(10);
const PROPERTIES: &str =
    "LoadState,ActiveState,SubState,MainPID,InvocationID,User,DynamicUser,Environment";

/// 分体部署的三个角色，顺序即启动顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedRole {
    Worker,
    Admin,
    Public,
}

impl ManagedRole {
    pub(crate) const START_ORDER: [Self; 3] = [Self::Worker, Self::Admin, Self::Public];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Admin => "admin",
            Self::Public => "public",
        }
    }
}

/// unit 的安全状态快照；Environment 只保留 AUCTION_CONFIG_FILE。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct UnitState {
    pub(crate) load_state: String,
    pub(crate) active_state: String,
    pub(crate) sub_state: String,
    pub(crate) main_pid: u32,
    pub(crate) invocation_id: String,
    #[serde(skip)]
    pub(crate) user: String,
    #[serde(skip)]
    pub(crate) dynamic_user: bool,
    pub(crate) config_file: Option<PathBuf>,
}

impl UnitState {
    pub(crate) fn running(&self) -> bool {
        self.load_state == "loaded" && self.active_state == "active" && self.main_pid > 0
    }

    /// 迁移与恢复只接受明确停止的 unit；转换中的 activating/deactivating 不算停止。
    pub(crate) fn stopped(&self) -> bool {
        self.load_state == "not-found"
            || matches!(self.active_state.as_str(), "inactive" | "failed")
    }
}

pub(crate) struct Systemctl {
    executable: PathBuf,
    units: [String; 3],
}

impl Systemctl {
    pub(crate) fn new(args: &UnitArgs) -> anyhow::Result<Self> {
        if !args.systemctl.is_absolute() {
            anyhow::bail!("--systemctl 必须是绝对路径");
        }
        let metadata = std::fs::metadata(&args.systemctl)
            .with_context(|| format!("找不到 systemctl：{}", args.systemctl.display()))?;
        if !metadata.is_file() {
            anyhow::bail!("--systemctl 必须指向可执行文件");
        }
        let units = [args.worker_unit.clone(), args.admin_unit.clone(), args.public_unit.clone()];
        for unit in &units {
            validate_unit(unit)?;
        }
        if units[0] == units[1] || units[1] == units[2] || units[0] == units[2] {
            anyhow::bail!("三个角色必须使用不同的 systemd unit");
        }
        Ok(Self { executable: args.systemctl.clone(), units })
    }

    pub(crate) fn unit(&self, role: ManagedRole) -> &str {
        match role {
            ManagedRole::Worker => &self.units[0],
            ManagedRole::Admin => &self.units[1],
            ManagedRole::Public => &self.units[2],
        }
    }

    pub(crate) async fn show(&self, role: ManagedRole) -> anyhow::Result<UnitState> {
        let output = self
            .run(&["show", "--property", PROPERTIES, "--", self.unit(role)], SHOW_TIMEOUT)
            .await
            .with_context(|| format!("读取 {} unit 状态失败", role.name()))?;
        parse_show(&output)
    }

    pub(crate) async fn start(&self, role: ManagedRole) -> anyhow::Result<()> {
        self.run(&["start", "--", self.unit(role)], START_TIMEOUT)
            .await
            .with_context(|| format!("启动 {} unit 失败", self.unit(role)))
            .map(drop)
    }

    pub(crate) async fn stop(&self, role: ManagedRole) -> anyhow::Result<()> {
        let value = self
            .run(
                &["show", "--property=TimeoutStopUSec", "--value", "--", self.unit(role)],
                SHOW_TIMEOUT,
            )
            .await?;
        let timeout = stop_timeout(&value)?
            .checked_add(STOP_RECEIPT_MARGIN)
            .context("unit stop receipt budget overflow")?;
        self.run(&["stop", "--", self.unit(role)], timeout)
            .await
            .with_context(|| format!("停止 {} unit 失败", self.unit(role)))
            .map(drop)
    }

    /// 超时会终止 systemctl 客户端，但已提交给 systemd 的 job 可能仍在执行，须重新读取状态。
    async fn run(&self, arguments: &[&str], timeout: Duration) -> anyhow::Result<String> {
        let child = Command::new(&self.executable)
            .args(arguments)
            .env("SYSTEMD_PAGER", "")
            .env("SYSTEMD_COLORS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("启动 systemctl 失败")?;
        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "systemctl 超过 {} 秒未返回；请重新核对 unit 状态",
                    timeout.as_secs()
                )
            })?
            .context("等待 systemctl 失败")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let line = stderr.lines().next().unwrap_or_default().chars().take(256);
            anyhow::bail!("systemctl 返回 {}：{}", output.status, line.collect::<String>());
        }
        String::from_utf8(output.stdout).context("systemctl 输出不是 UTF-8")
    }
}

/// systemd unit 名称闭集：防止参数注入或误把模板/路径当作 unit。
fn validate_unit(unit: &str) -> anyhow::Result<()> {
    let valid = unit.ends_with(".service")
        && unit.len() <= 256
        && !unit.starts_with('-')
        && unit.bytes().all(|byte| byte.is_ascii_alphanumeric() || b":_.@-\\".contains(&byte));
    if !valid {
        anyhow::bail!("systemd unit 名称无效：{unit}");
    }
    Ok(())
}

fn parse_show(output: &str) -> anyhow::Result<UnitState> {
    let mut state = UnitState {
        load_state: String::new(),
        active_state: String::new(),
        sub_state: String::new(),
        main_pid: 0,
        invocation_id: String::new(),
        user: String::new(),
        dynamic_user: false,
        config_file: None,
    };
    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        match key {
            "LoadState" => state.load_state = value.to_owned(),
            "ActiveState" => state.active_state = value.to_owned(),
            "SubState" => state.sub_state = value.to_owned(),
            "MainPID" => state.main_pid = value.parse().unwrap_or(0),
            "InvocationID" => state.invocation_id = value.to_owned(),
            "User" => state.user = value.to_owned(),
            "DynamicUser" => state.dynamic_user = value == "yes",
            "Environment" => {
                state.config_file = value
                    .split_whitespace()
                    .find_map(|entry| entry.strip_prefix("AUCTION_CONFIG_FILE="))
                    .map(PathBuf::from);
            }
            _ => {}
        }
    }
    if state.load_state.is_empty() || state.active_state.is_empty() {
        anyhow::bail!("systemctl show 输出缺少 LoadState/ActiveState");
    }
    Ok(state)
}

/// unit 的 AUCTION_CONFIG_FILE 必须与预检使用的角色文件是同一文件。
pub(crate) fn require_config_binding(
    role: ManagedRole,
    state: &UnitState,
    expected: &Path,
) -> anyhow::Result<()> {
    let configured = state
        .config_file
        .as_deref()
        .with_context(|| format!("{} unit 未设置 Environment=AUCTION_CONFIG_FILE", role.name()))?;
    let configured = std::fs::canonicalize(configured)
        .with_context(|| format!("解析 {} unit 的 AUCTION_CONFIG_FILE 失败", role.name()))?;
    let expected = std::fs::canonicalize(expected)
        .with_context(|| format!("解析 {} 预检角色文件失败", role.name()))?;
    if configured != expected {
        anyhow::bail!(
            "{} unit 的 AUCTION_CONFIG_FILE（{}）与 --config-dir 中的角色文件（{}）不一致",
            role.name(),
            configured.display(),
            expected.display()
        );
    }
    Ok(())
}

/// systemctl 将 USEC 属性格式化为有限时间片（如 1min 30s）；拒绝 infinity 和未知单位。
/// 只解析单个指定属性，绝不读取或打印其他 unit 配置。
fn stop_timeout(value: &str) -> anyhow::Result<Duration> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 || value == "infinity" {
        anyhow::bail!("TimeoutStopUSec 必须是有限时长");
    }
    let mut micros = 0_u64;
    for part in value.split_whitespace() {
        let split = part.find(|c: char| !c.is_ascii_digit()).unwrap_or(part.len());
        let (number, unit) = part.split_at(split);
        let amount: u64 = number.parse().context("解析 TimeoutStopUSec 数值失败")?;
        let scale = match unit {
            "" | "us" | "µs" => 1,
            "ms" => 1_000,
            "s" => 1_000_000,
            "min" => 60_000_000,
            "h" => 3_600_000_000,
            "d" => 86_400_000_000,
            _ => anyhow::bail!("TimeoutStopUSec 单位不受支持"),
        };
        micros = amount
            .checked_mul(scale)
            .and_then(|part| micros.checked_add(part))
            .context("TimeoutStopUSec 超出可表示范围")?;
    }
    if micros == 0 {
        anyhow::bail!("TimeoutStopUSec 必须大于零");
    }
    Ok(Duration::from_micros(micros))
}
