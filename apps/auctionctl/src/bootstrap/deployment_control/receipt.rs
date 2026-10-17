//! systemd 启动顺序回执的私有文件边界；只绑定受控 unit 的进程身份和观察时间，不保存
//! TOML、环境变量或命令输出。

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::{fs::OpenOptionsExt, fs::PermissionsExt},
    path::Path,
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::systemd::{ManagedRole, START_ORDER, UnitSnapshot};

const RECEIPT_FILE: &str = "startup-order.v1.json";
const RECEIPT_SCHEMA: &str = "auction-systemd-startup-order-v1";

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct StartupReceipt {
    schema: String,
    controller_version: String,
    generated_at: String,
    mode: String,
    roles: Vec<RoleReceipt>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct RoleReceipt {
    role: String,
    unit: String,
    main_pid: u32,
    invocation_id: String,
    started_monotonic_micros: u64,
    observed_started_at: String,
    observed_ready_at: String,
}

impl RoleReceipt {
    pub(super) fn new(
        role: ManagedRole,
        unit: &str,
        snapshot: UnitSnapshot,
        observed_started_at: String,
        observed_ready_at: String,
    ) -> Self {
        Self {
            role: role.name().to_owned(),
            unit: unit.to_owned(),
            main_pid: snapshot.main_pid,
            invocation_id: snapshot.invocation_id,
            started_monotonic_micros: snapshot.started_monotonic_micros,
            observed_started_at,
            observed_ready_at,
        }
    }
}

pub(super) fn publish(
    state_dir: &Path,
    mode: &str,
    roles: Vec<RoleReceipt>,
    generated_at: String,
) -> anyhow::Result<()> {
    if roles.len() != START_ORDER.len() {
        anyhow::bail!("启动回执缺少角色记录")
    }
    let receipt = StartupReceipt {
        schema: RECEIPT_SCHEMA.to_owned(),
        controller_version: env!("CARGO_PKG_VERSION").to_owned(),
        generated_at,
        mode: mode.to_owned(),
        roles,
    };
    validate(&receipt)?;
    let mut contents = serde_json::to_vec_pretty(&receipt).context("编码启动回执失败")?;
    contents.push(b'\n');
    let target = state_dir.join(RECEIPT_FILE);
    if let Ok(metadata) = fs::symlink_metadata(&target)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        anyhow::bail!("启动回执目标不是安全普通文件：{}", target.display())
    }
    let temporary = state_dir.join(format!(".{RECEIPT_FILE}.{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> anyhow::Result<()> {
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let mut file = options.open(&temporary).context("创建启动回执临时文件失败")?;
        file.write_all(&contents).context("写入启动回执失败")?;
        file.sync_all().context("同步启动回执失败")?;
        fs::rename(&temporary, &target).context("原子发布启动回执失败")?;
        File::open(state_dir)
            .and_then(|directory| directory.sync_all())
            .context("同步启动回执目录失败")?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

pub(super) fn read(state_dir: &Path) -> anyhow::Result<Option<StartupReceipt>> {
    let path = state_dir.join(RECEIPT_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("读取启动回执元数据失败"),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("启动回执不是安全普通文件")
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("启动回执不得允许 group/other 访问")
    }
    if metadata.len() > 64 * 1024 {
        anyhow::bail!("启动回执超过 64 KiB 上限")
    }
    let contents = fs::read(&path).context("读取启动回执失败")?;
    let receipt: StartupReceipt = serde_json::from_slice(&contents).context("解析启动回执失败")?;
    validate(&receipt)?;
    Ok(Some(receipt))
}

pub(super) fn remove(state_dir: &Path) -> anyhow::Result<()> {
    let path = state_dir.join(RECEIPT_FILE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(&path).context("删除启动回执失败")?;
            Ok(())
        }
        Ok(_) => anyhow::bail!("拒绝删除非普通文件启动回执"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("读取启动回执元数据失败"),
    }
}

pub(super) fn receipt_match(
    receipt: &StartupReceipt,
    role: ManagedRole,
    unit: &str,
    snapshot: &UnitSnapshot,
) -> &'static str {
    receipt
        .roles
        .iter()
        .find(|entry| entry.role == role.name())
        .filter(|entry| {
            entry.unit == unit
                && entry.main_pid == snapshot.main_pid
                && entry.invocation_id == snapshot.invocation_id
                && entry.started_monotonic_micros == snapshot.started_monotonic_micros
        })
        .map_or("stale", |_| "matched")
}

fn validate(receipt: &StartupReceipt) -> anyhow::Result<()> {
    if receipt.schema != RECEIPT_SCHEMA
        || receipt.controller_version.is_empty()
        || !matches!(receipt.mode.as_str(), "started" | "adopted")
        || receipt.roles.len() != START_ORDER.len()
        || chrono::DateTime::parse_from_rfc3339(&receipt.generated_at).is_err()
    {
        anyhow::bail!("启动回执头部契约无效")
    }
    let mut previous_started = 0;
    for (expected, role) in START_ORDER.into_iter().zip(&receipt.roles) {
        if role.role != expected.name()
            || role.unit.is_empty()
            || role.main_pid == 0
            || role.invocation_id.len() != 32
            || !role.invocation_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || role.started_monotonic_micros == 0
            || (previous_started > 0 && role.started_monotonic_micros <= previous_started)
        {
            anyhow::bail!("启动回执角色记录无效")
        }
        let started = chrono::DateTime::parse_from_rfc3339(&role.observed_started_at)
            .context("启动回执 observed_started_at 无效")?;
        let ready = chrono::DateTime::parse_from_rfc3339(&role.observed_ready_at)
            .context("启动回执 observed_ready_at 无效")?;
        if ready < started {
            anyhow::bail!("启动回执 readiness 时间早于启动观察时间")
        }
        previous_started = role.started_monotonic_micros;
    }
    Ok(())
}
