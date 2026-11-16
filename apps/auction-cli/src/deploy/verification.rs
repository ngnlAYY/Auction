//! 部署前后的制品与运行状态核对。

use std::path::Path;

use anyhow::Context;

use super::artifact;
use crate::receipt;
use crate::service::{ManagedRole, Systemctl, UnitState};
use crate::support::digest::sha256_path;
use crate::support::files;

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

/// 安装中途失败时安装目录可能新旧混合：按实际文件计算三角色程序摘要，不能用制品摘要冒充；
/// 读不到的程序只记录路径，摘要与字节数留空，不伪造已验证身份。
pub(super) fn install_dir_artifacts(install_dir: &Path) -> Vec<receipt::Artifact> {
    ManagedRole::START_ORDER
        .into_iter()
        .map(|role| {
            let path = install_dir.join(role_binary(role));
            let digest = sha256_path(&path).ok().filter(|(_, bytes)| *bytes > 0);
            receipt::Artifact {
                role: role.name().to_owned(),
                path: Some(path.display().to_string()),
                sha256: digest.as_ref().map(|(sha256, _)| sha256.clone()),
                bytes: digest.map(|(_, bytes)| bytes),
            }
        })
        .collect()
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

pub(super) fn require_artifact_cli(artifact: &artifact::Artifact) -> anyhow::Result<()> {
    let current = std::env::current_exe().context("读取当前 auction-cli 路径失败")?;
    let (sha256, _) = sha256_path(&current).context("计算当前 auction-cli 摘要失败")?;
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

/// 停机前按停机备份写入时的同一口径只读核对 `--backup-dir`：已存在、非符号链接、禁止
/// group/other 写入，且归当前账号或 root 所有（root 部署只能是 root）。不创建目录，dry-run
/// 同样执行；否则属主不符要到停机后写停机备份时才被拒绝。
pub(super) fn require_backup_dir(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("--backup-dir 必须是绝对路径");
    }
    files::require_private_directory(path, "备份目录")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use clap::Parser;

    use super::require_backup_dir;
    use crate::cli::{Cli, Command};

    /// clap 对 `--backup-dir` 接受任意路径（dry-run 也一样），校验全由 `require_backup_dir`
    /// 完成：相对、缺失或 group 可写的目录都被拒绝。`deploy::run` 在 dry-run 分支之前、任何锁、
    /// systemctl 与数据库访问之前无条件调用它。
    #[test]
    fn backup_dir_gate_rejects_relative_missing_and_group_writable_paths() {
        let cli = Cli::try_parse_from([
            "auction-cli",
            "deploy",
            "/srv/artifact",
            "--owner-secrets",
            "/etc/owner.toml",
            "--backup-dir",
            "relative/backups",
            "--dry-run",
        ])
        .expect("参数可以解析");
        let Command::Deploy(args) = cli.command else { panic!("应解析为 deploy") };
        assert!(args.dry_run);
        assert!(require_backup_dir(&args.backup_dir).is_err());

        let root =
            std::env::temp_dir().join(format!("auction-backup-dir-{}", uuid::Uuid::new_v4()));
        assert!(require_backup_dir(&root).is_err(), "不存在的目录必须拒绝");
        fs::create_dir(&root).expect("建立临时目录");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o770)).expect("设置权限");
        assert!(require_backup_dir(&root).is_err(), "group 可写必须拒绝");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("设置权限");
        assert!(require_backup_dir(&root).is_ok());
        assert!(require_backup_dir(Path::new("relative")).is_err());
        let link = root.with_extension("link");
        std::os::unix::fs::symlink(&root, &link).expect("符号链接");
        assert!(require_backup_dir(&link).is_err(), "符号链接必须拒绝");
        fs::remove_file(&link).expect("清理符号链接");
        fs::remove_dir(&root).expect("清理临时目录");
    }
}
