//! systemctl 的有界异步调用；只读取 unit 属性或请求启停，不解析或回显 unit 环境中的其他值。

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::Context;
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

use crate::cli::UnitArgs;

const SHOW_TIMEOUT: Duration = Duration::from_secs(10);
const START_TIMEOUT: Duration = Duration::from_secs(90);
/// systemd unit 自己的有限停止预算之外，给客户端回执额外留十秒。
const STOP_RECEIPT_MARGIN: Duration = Duration::from_secs(10);
const MAX_SYSTEMCTL_STDOUT_BYTES: usize = 64 * 1024;
const MAX_SYSTEMCTL_STDERR_BYTES: usize = 16 * 1024;
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
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            anyhow::bail!("--systemctl 必须指向有执行权限的普通文件");
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
        let mut child = Command::new(&self.executable)
            .args(arguments)
            .env("SYSTEMD_PAGER", "")
            .env("SYSTEMD_COLORS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("启动 systemctl 失败")?;
        let stdout = child.stdout.take().context("systemctl 标准输出管道不可用")?;
        let stderr = child.stderr.take().context("systemctl 标准错误管道不可用")?;
        tokio::time::timeout(timeout, async {
            let (status, stdout, stderr) = tokio::join!(
                child.wait(),
                read_bounded(stdout, MAX_SYSTEMCTL_STDOUT_BYTES),
                read_bounded(stderr, MAX_SYSTEMCTL_STDERR_BYTES),
            );
            let status = status.context("等待 systemctl 失败")?;
            let (stdout, stdout_exceeded) = stdout.context("读取 systemctl 标准输出失败")?;
            let (stderr, stderr_exceeded) = stderr.context("读取 systemctl 标准错误失败")?;
            if stdout_exceeded || stderr_exceeded {
                anyhow::bail!("systemctl 输出超过有界读取上限");
            }
            if !status.success() {
                let stderr = String::from_utf8_lossy(&stderr);
                let line = stderr.lines().next().unwrap_or_default().chars().take(256);
                anyhow::bail!("systemctl 返回 {}：{}", status, line.collect::<String>());
            }
            String::from_utf8(stdout).context("systemctl 输出不是 UTF-8")
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!("systemctl 超过 {} 秒未返回；请重新核对 unit 状态", timeout.as_secs())
        })?
    }
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    maximum: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(maximum.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut exceeded = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok((output, exceeded));
        }
        let accepted = maximum.saturating_sub(output.len()).min(count);
        output.extend_from_slice(&buffer[..accepted]);
        exceeded |= accepted < count;
    }
}

/// systemd unit 名称闭集：防止参数注入，并拒绝空基名与模板（`foo@.service`）：模板能通过
/// `show` 的装载检查，却只会在停机后的 `start` 才失败。
fn validate_unit(unit: &str) -> anyhow::Result<()> {
    let stem = unit.strip_suffix(".service").filter(|stem| !stem.is_empty());
    let name_and_instance = stem.is_some_and(|stem| match stem.split_once('@') {
        Some((name, instance)) => {
            !name.is_empty() && !instance.is_empty() && !instance.contains('@')
        }
        None => true,
    });
    let valid = unit.len() <= 256
        && !unit.starts_with('-')
        && name_and_instance
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
    let mut has_sub_state = false;
    let mut has_main_pid = false;
    let mut has_invocation_id = false;
    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        match key {
            "LoadState" => state.load_state = value.to_owned(),
            "ActiveState" => state.active_state = value.to_owned(),
            "SubState" => {
                state.sub_state = value.to_owned();
                has_sub_state = true;
            }
            "MainPID" => {
                state.main_pid = value.parse().context("systemctl MainPID 格式无效")?;
                has_main_pid = true;
            }
            "InvocationID" => {
                state.invocation_id = value.to_owned();
                has_invocation_id = true;
            }
            "User" => state.user = value.to_owned(),
            "DynamicUser" => state.dynamic_user = value == "yes",
            "Environment" => state.config_file = environment_config_file(value),
            _ => {}
        }
    }
    if state.load_state.is_empty()
        || state.active_state.is_empty()
        || !has_sub_state
        || !has_main_pid
        || !has_invocation_id
    {
        anyhow::bail!("systemctl show 输出缺少必要状态属性");
    }
    if state.active_state == "active" && (state.main_pid == 0 || state.invocation_id.is_empty()) {
        anyhow::bail!("systemctl active 状态缺少有效 MainPID 或 InvocationID");
    }
    Ok(state)
}

fn environment_config_file(value: &str) -> Option<PathBuf> {
    let mut entries = Vec::new();
    let mut entry = String::new();
    let mut quoted = false;
    let mut quote_closed = false;
    let mut chars = value.chars().peekable();
    let mut token_started = false;
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted => {
                quoted = false;
                quote_closed = true;
            }
            '"' if !quote_closed && !token_started => {
                quoted = true;
                token_started = true;
            }
            '"' => return None,
            '\\' if !quote_closed => {
                let next = chars.next()?;
                match next {
                    's' => entry.push(' '),
                    't' => entry.push('\t'),
                    'n' => entry.push('\n'),
                    'r' => entry.push('\r'),
                    'x' => {
                        let high = chars.next()?.to_digit(16)?;
                        let low = chars.next()?.to_digit(16)?;
                        entry.push(char::from_u32(high * 16 + low)?);
                    }
                    '\\' | '"' => entry.push(next),
                    _ => return None,
                }
            }
            '\\' => return None,
            character if character.is_whitespace() && !quoted => {
                if token_started {
                    entries.push(std::mem::take(&mut entry));
                }
                token_started = false;
                quote_closed = false;
            }
            other if !quote_closed => {
                token_started = true;
                entry.push(other);
            }
            _ => return None,
        }
    }
    if quoted {
        return None;
    }
    if token_started {
        entries.push(entry);
    }
    entries
        .into_iter()
        .find_map(|entry| entry.strip_prefix("AUCTION_CONFIG_FILE=").map(PathBuf::from))
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
    let mut component_count = 0_usize;
    let mut previous_scale = u64::MAX;
    for part in value.split_whitespace() {
        let split =
            part.find(|c: char| !c.is_ascii_digit()).context("TimeoutStopUSec 分量缺少时间单位")?;
        let (number, unit) = part.split_at(split);
        if number.is_empty() {
            anyhow::bail!("TimeoutStopUSec 分量缺少数值");
        }
        let amount: u64 = number.parse().context("解析 TimeoutStopUSec 数值失败")?;
        if amount == 0 {
            anyhow::bail!("TimeoutStopUSec 不允许零时长分量");
        }
        let scale = match unit {
            "us" | "µs" => 1,
            "ms" => 1_000,
            "s" => 1_000_000,
            "min" => 60_000_000,
            "h" => 3_600_000_000,
            "d" => 86_400_000_000,
            _ => anyhow::bail!("TimeoutStopUSec 单位不受支持"),
        };
        if scale >= previous_scale {
            anyhow::bail!("TimeoutStopUSec 时间单位顺序或重复分量无效");
        }
        previous_scale = scale;
        component_count += 1;
        micros = amount
            .checked_mul(scale)
            .and_then(|part| micros.checked_add(part))
            .context("TimeoutStopUSec 超出可表示范围")?;
    }
    if component_count == 0 {
        anyhow::bail!("TimeoutStopUSec 缺少时间分量");
    }
    if micros == 0 {
        anyhow::bail!("TimeoutStopUSec 必须大于零");
    }
    Ok(Duration::from_micros(micros))
}

#[cfg(test)]
mod tests {
    use super::validate_unit;

    /// unit 名称闭集拒绝空基名与模板：它们能通过 `show`，却只会在停机后的 `start` 才失败。
    #[test]
    fn unit_names_reject_empty_bases_and_templates() {
        for valid in ["auction-worker.service", "auction@blue.service"] {
            validate_unit(valid).expect(valid);
        }
        for invalid in [
            ".service",
            "-worker.service",
            "auction@.service",
            "auction@a@b.service",
            "worker.socket",
            "worker",
            "/opt/worker.service",
        ] {
            assert!(validate_unit(invalid).is_err(), "{invalid}");
        }
    }
}
