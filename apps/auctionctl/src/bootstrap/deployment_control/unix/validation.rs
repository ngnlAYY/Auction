//! Unix 部署控制器的路径、配置和 systemd 环境校验。

use std::{
    fs,
    os::unix::{fs::MetadataExt, fs::PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::Context;
use chrono::{SecondsFormat, Utc};

use super::super::systemd::{ManagedRole, UnitSnapshot};

pub(in crate::bootstrap::deployment_control::unix) fn validate_private_state_dir(
    path: &Path,
) -> anyhow::Result<PathBuf> {
    if !path.is_absolute() || path == Path::new("/") {
        anyhow::bail!("部署控制状态目录必须是非根绝对路径")
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("规范化部署控制状态目录失败：{}", path.display()))?;
    if canonical != path {
        anyhow::bail!("部署控制状态目录必须是规范化、无符号链接的绝对路径")
    }
    let metadata = fs::symlink_metadata(&canonical).context("读取部署控制状态目录失败")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("部署控制状态目录必须是无符号链接的目录")
    }
    // systemd StateDirectory 与手工控制都要求操作者拥有目录，避免特权进程写入低权限用户可替换的路径。
    // SAFETY: geteuid 不接收指针，也不访问 Rust 管理的内存，只返回当前进程的有效 UID。
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        anyhow::bail!("部署控制状态目录必须归当前有效用户所有")
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("部署控制状态目录不得允许 group/other 访问；要求权限至多 0700")
    }
    Ok(canonical)
}

pub(in crate::bootstrap::deployment_control::unix) fn validate_config_dir(
    path: &Path,
) -> anyhow::Result<PathBuf> {
    if !path.is_absolute() || path == Path::new("/") {
        anyhow::bail!("deployment start 的 config-dir 必须是非根绝对路径")
    }
    let display = path.to_str().ok_or_else(|| anyhow::anyhow!("config-dir 必须是 UTF-8 路径"))?;
    if display.chars().any(|character| character.is_whitespace() || character.is_control()) {
        anyhow::bail!("config-dir 不能包含空白或控制字符")
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("规范化 config-dir 失败：{}", path.display()))?;
    let metadata = fs::symlink_metadata(&canonical).context("读取 config-dir 元数据失败")?;
    if canonical != path || !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("config-dir 必须是规范化、无符号链接的现有目录")
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!("config-dir 不得允许 group/other 写入")
    }
    Ok(canonical)
}

pub(in crate::bootstrap::deployment_control::unix) fn validate_deployment_config_file(
    path: &Path,
) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("读取部署主配置元数据失败：{}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("部署主配置必须是无符号链接的普通文件：{}", path.display())
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!("部署主配置不得允许 group/other 写入：{}", path.display())
    }
    Ok(())
}

pub(in crate::bootstrap::deployment_control::unix) fn ensure_unit_config(
    role: ManagedRole,
    unit: &str,
    config_path: &Path,
    snapshot: &UnitSnapshot,
) -> anyhow::Result<()> {
    // 外部环境源或最终删除规则可覆盖 Environment；不解析通用 systemd 转义与间接输入。
    for (setting, value) in [
        ("EnvironmentFiles", &snapshot.environment_files),
        ("UnsetEnvironment", &snapshot.unset_environment),
        ("PassEnvironment", &snapshot.pass_environment),
        ("PAMName", &snapshot.pam_name),
    ] {
        if !value.trim().is_empty() {
            anyhow::bail!(
                "{} unit {} 配置了不支持的 {}，无法证明最终 AUCTION_CONFIG_FILE 绑定",
                role.name(),
                unit,
                setting,
            );
        }
    }
    let mut configured = None;
    for entry in snapshot.environment.split_ascii_whitespace() {
        let entry = if entry.starts_with('"') && entry.ends_with('"') && entry.len() >= 2 {
            &entry[1..entry.len() - 1]
        } else {
            entry
        };
        // 拒绝引号内空白和转义，防止另一个变量的值伪装成独立配置赋值；错误不回显环境值。
        if entry.contains(['"', '\\', '\'']) {
            anyhow::bail!("systemd Environment 包含不支持的引号或转义，无法证明配置绑定");
        }
        let (name, value) = entry
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("systemd Environment 不是受支持的简单赋值"))?;
        if name == "AUCTION_CONFIG_FILE" && configured.replace(value).is_some() {
            anyhow::bail!("systemd Environment 重复声明 AUCTION_CONFIG_FILE");
        }
    }
    if configured != config_path.to_str() {
        anyhow::bail!(
            "{} unit {} 未声明预期的 AUCTION_CONFIG_FILE={}；请先 daemon-reload",
            role.name(),
            unit,
            config_path.display(),
        )
    }
    Ok(())
}

pub(in crate::bootstrap::deployment_control::unix) fn canonical_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
