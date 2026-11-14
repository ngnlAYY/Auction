//! 部署前后的制品与运行状态核对。

use std::path::Path;

use anyhow::Context;

use super::artifact;
use crate::service::{ManagedRole, Systemctl, UnitState};
use crate::{receipt, service};

/// 已安装制品的静态回执；启动失败时没有可核对的运行进程，只能用制品摘要证明回滚目标。
pub(super) fn installed_artifacts(
    artifact: &artifact::Artifact,
) -> anyhow::Result<Vec<receipt::Artifact>> {
    let mut artifacts = Vec::with_capacity(ManagedRole::START_ORDER.len());
    for role in ManagedRole::START_ORDER {
        let binary = artifact
            .binary(role_binary(role))
            .with_context(|| format!("制品缺少 {} 程序", role.name()))?;
        artifacts.push(receipt::Artifact {
            role: role.name().to_owned(),
            path: Some(binary.path.display().to_string()),
            sha256: Some(binary.sha256.clone()),
            bytes: Some(binary.bytes),
        });
    }
    Ok(artifacts)
}

/// 运行中的程序摘要必须与制品完全一致；这也是内嵌静态资源已生效的证据。
pub(super) async fn verify_running_artifacts(
    systemctl: &Systemctl,
    artifact: &artifact::Artifact,
) -> anyhow::Result<Vec<receipt::Artifact>> {
    let mut artifacts = Vec::with_capacity(ManagedRole::START_ORDER.len());
    for role in ManagedRole::START_ORDER {
        let state = systemctl.show(role).await?;
        if !state.running() {
            anyhow::bail!("{} 未运行，无法核对程序摘要", role.name());
        }
        let record = receipt::artifact_from_process(role.name(), state.main_pid);
        let expected = artifact
            .binary(role_binary(role))
            .with_context(|| format!("制品缺少 {} 程序", role.name()))?;
        if record.sha256.as_deref() != Some(expected.sha256.as_str()) {
            anyhow::bail!(
                "{} 运行的程序摘要与制品不一致；请核对安装目录与 unit ExecStart",
                role.name()
            );
        }
        artifacts.push(record);
    }
    Ok(artifacts)
}

pub(crate) const fn role_binary(role: ManagedRole) -> &'static str {
    match role {
        ManagedRole::Worker => "auction-worker",
        ManagedRole::Admin => "auction-admin-api",
        ManagedRole::Public => "auction-public-api",
    }
}

pub(crate) async fn unit_states(systemctl: &Systemctl) -> anyhow::Result<[UnitState; 3]> {
    let mut states = Vec::with_capacity(3);
    for role in ManagedRole::START_ORDER {
        states.push(systemctl.show(role).await?);
    }
    states.try_into().ok().context("三角色状态数量不一致")
}

pub(crate) fn running_roles(states: &[UnitState; 3]) -> Vec<(&'static str, u32)> {
    ManagedRole::START_ORDER
        .iter()
        .zip(states)
        .filter(|(_, state)| state.running())
        .map(|(role, state)| (role.name(), state.main_pid))
        .collect()
}

/// 三个 unit 必须使用同一个固定服务账号；DynamicUser 或账号不一致都不能原子安装。
pub(crate) fn shared_service_uid(states: &[UnitState; 3]) -> anyhow::Result<u32> {
    let mut uid = None;
    for (role, state) in ManagedRole::START_ORDER.iter().zip(states) {
        if state.dynamic_user {
            anyhow::bail!("{} unit 使用 DynamicUser，无法为程序设置稳定属主", role.name());
        }
        let current = service::service_uid(&state.user)?;
        match uid {
            Some(expected) if expected != current => {
                anyhow::bail!("三角色 unit 必须使用同一个服务账号才能原子安装程序");
            }
            Some(_) => {}
            None => uid = Some(current),
        }
    }
    uid.context("无法确定服务账号")
}

pub(super) fn require_artifact_cli(artifact: &artifact::Artifact) -> anyhow::Result<()> {
    let current = std::env::current_exe().context("读取当前 auction-cli 路径失败")?;
    let (sha256, _) = artifact::sha256_path(&current).context("计算当前 auction-cli 摘要失败")?;
    if sha256 != artifact.cli_sha256() {
        anyhow::bail!(
            "deploy 必须由制品内的 auction-cli 执行（{}）；当前程序摘要与制品不一致",
            artifact.binary("auction-cli").map_or_else(
                || "bin/auction-cli".to_owned(),
                |binary| binary.path.display().to_string()
            )
        );
    }
    Ok(())
}

/// 备份目录必须已存在、非符号链接，且不允许 group/other 写入。
pub(super) fn require_backup_dir(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("--backup-dir 必须是绝对路径");
    }
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("读取备份目录失败：{}", path.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("备份目录必须是非符号链接目录：{}", path.display());
    }
    use std::os::unix::fs::MetadataExt;
    if metadata.mode() & 0o022 != 0 {
        anyhow::bail!("备份目录不得允许 group/other 写入：{}", path.display());
    }
    Ok(())
}
