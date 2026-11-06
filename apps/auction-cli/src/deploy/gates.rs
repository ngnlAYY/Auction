//! 部署前的数据库与外部程序门禁：在途发送、迁移版本、旧 CLI 版本与备份双 CLI 校验。
//! 所有查询都是有界只读；外部程序以固定参数与超时调用，不经过 shell。

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::Context;
use auction_config::AppConfig;
use auction_storage::DatabaseConnectionPurpose;
use tokio::process::Command;

use crate::backup::BackupManifest;
use crate::database::{DatabaseTarget, preflight};

/// 旧 CLI 单次调用的硬上限；`db status` 与 `backup verify` 都是短命令。
const CLI_TIMEOUT: Duration = Duration::from_secs(300);
/// 在途发送门禁的轮询间隔。
const GATE_POLL: Duration = Duration::from_secs(1);

/// 等待已越过 sending 围栏的投递在租约内完成；超时只拒绝部署，不修改队列或租约。
pub(crate) async fn require_no_in_flight_sends(
    config: &AppConfig,
    timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    let store = crate::database::connect(config, DatabaseConnectionPurpose::CliDeploy).await?;
    let result = async {
        let auction_storage::DatabaseStore::Postgres(postgres) = &store;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let snapshot =
                postgres.collect_in_flight_sends().await.context("读取在途发送证据失败")?;
            let report = serde_json::to_value(&snapshot)?;
            if snapshot.is_empty() {
                return Ok(report);
            }
            if tokio::time::Instant::now() >= deadline {
                let ids = snapshot
                    .mail_sends
                    .iter()
                    .map(|send| format!("delivery:{}", send.delivery_id))
                    .chain(
                        snapshot
                            .pm_sends
                            .iter()
                            .map(|send| format!("lot_watch_pm:{}", send.notification_id)),
                    )
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!(
                    "仍有 {} 笔已越过 sending 围栏的投递在途（{ids}）；等待完成或人工核对后重试，\
                     不得以停机打断外部写入",
                    snapshot.len()
                );
            }
            tokio::time::sleep(GATE_POLL).await;
        }
    }
    .await;
    store.postgres_pool().close().await;
    result
}

/// 读取当前数据库迁移版本；没有迁移历史说明不是 Auction 数据库，不能部署。
pub(crate) async fn database_version(config: &AppConfig) -> anyhow::Result<i64> {
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    let target = DatabaseTarget::parse(url)?;
    let store = crate::database::connect(config, DatabaseConnectionPurpose::CliDeploy).await?;
    let result = preflight(&store, &target.database).await;
    store.postgres_pool().close().await;
    let preflight = result?;
    let migration = preflight.migration.context("数据库没有 Auction 迁移历史，拒绝部署")?;
    if !migration.current && migration.latest_version == 0 {
        anyhow::bail!("数据库没有已执行的迁移，拒绝部署");
    }
    Ok(migration.latest_version)
}

/// 用指定 CLI 离线校验备份；新旧 CLI 都必须通过，才算恢复点可用。
pub(crate) async fn verify_backup_with_cli(
    cli: &Path,
    config_dir: &Path,
    manifest: &Path,
    database: &str,
) -> anyhow::Result<()> {
    let manifest = manifest.to_str().context("备份 manifest 路径不是 UTF-8")?.to_owned();
    let database = database.to_owned();
    run_cli(
        cli,
        config_dir,
        &["backup", "verify", "--manifest", &manifest, "--database", &database],
    )
    .await
    .with_context(|| format!("{} 未能校验本次备份", cli.display()))?;
    Ok(())
}

/// 创建停机恢复点；调用方负责在输出中只暴露安全摘要。
pub(crate) async fn create_backup(
    config: &AppConfig,
    database: &str,
    output: &Path,
) -> anyhow::Result<BackupManifest> {
    crate::backup::create_backup(config, database, output).await
}

/// 回退门禁：高于旧程序的迁移必须连续、成功且逐条登记为 compatible。
pub(crate) async fn require_compatible_suffix(
    config: &AppConfig,
    from_version: i64,
) -> anyhow::Result<serde_json::Value> {
    if from_version < 0 {
        anyhow::bail!("旧程序期望版本无效");
    }
    let store = crate::database::connect(config, DatabaseConnectionPurpose::CliDeploy).await?;
    let result = async {
        let auction_storage::DatabaseStore::Postgres(postgres) = &store;
        postgres.collect_migration_suffix(from_version).await.context("读取迁移兼容后缀失败")
    }
    .await;
    store.postgres_pool().close().await;
    let suffix = result?;
    if suffix.current_version < from_version {
        anyhow::bail!("数据库版本低于旧程序期望版本，无法回退");
    }
    let report = serde_json::to_value(&suffix)?;
    if suffix.current_version == from_version {
        return Ok(report);
    }
    let mut previous = from_version;
    for entry in &suffix.entries {
        if previous.checked_add(1) != Some(entry.version)
            || !entry.success
            || entry.level.as_deref() != Some("compatible")
        {
            anyhow::bail!(
                "数据库包含未登记为 compatible 的迁移（版本 {}）；不能换回旧程序，\
                 请按 ops/deploy/RECOVERY.md 恢复升级前备份并核对历史",
                entry.version
            );
        }
        previous = entry.version;
    }
    if previous != suffix.current_version {
        anyhow::bail!("迁移兼容后缀未覆盖到数据库版本 {}，不能回退", suffix.current_version);
    }
    Ok(report)
}

pub(crate) fn database_name(config: &AppConfig) -> anyhow::Result<String> {
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    Ok(DatabaseTarget::parse(url)?.database)
}

/// 安装目录：显式参数优先；否则由当前运行三角色的 `/proc/<pid>/exe` 推导，三者必须同目录。
pub(crate) fn resolve_install_dir(
    explicit: Option<&Path>,
    running: &[(&'static str, u32)],
) -> anyhow::Result<PathBuf> {
    let mut derived: Option<PathBuf> = None;
    for (role, pid) in running {
        // 已停止的角色没有可核对的程序；启动前部署与停机后回退都可能遇到这种状态。
        if *pid == 0 {
            continue;
        }
        let executable = std::fs::read_link(format!("/proc/{pid}/exe"))
            .with_context(|| format!("读取 {role} 进程可执行文件失败"))?;
        let directory = executable.parent().context("运行中的程序路径缺少父目录")?.to_path_buf();
        match &derived {
            Some(expected) if *expected != directory => anyhow::bail!(
                "三角色程序不在同一目录（{} 与 {}）；请显式传入 --install-dir",
                expected.display(),
                directory.display()
            ),
            Some(_) => {}
            None => derived = Some(directory),
        }
    }
    match (explicit, derived) {
        (Some(path), None) => Ok(std::path::absolute(path).context("解析 --install-dir 失败")?),
        (Some(path), Some(derived)) => {
            if std::path::absolute(path).context("解析 --install-dir 失败")? != derived {
                anyhow::bail!(
                    "--install-dir（{}）与当前运行程序目录（{}）不一致",
                    path.display(),
                    derived.display()
                );
            }
            Ok(derived)
        }
        (None, Some(derived)) => Ok(derived),
        (None, None) => {
            anyhow::bail!("没有运行中的角色，无法推导安装目录；请显式传入 --install-dir")
        }
    }
}

async fn run_cli(
    cli: &Path,
    config_dir: &Path,
    arguments: &[&str],
) -> anyhow::Result<std::process::Output> {
    let config_dir = config_dir.to_str().context("配置目录不是 UTF-8")?;
    let child = Command::new(cli)
        .arg("--config-dir")
        .arg(config_dir)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("启动 {} 失败", cli.display()))?;
    let output = tokio::time::timeout(CLI_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|error| {
            let _ = error;
            anyhow::anyhow!("{} 超过 {} 秒未返回", cli.display(), CLI_TIMEOUT.as_secs())
        })?
        .with_context(|| format!("等待 {} 失败", cli.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line: String = stderr
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|character| !character.is_control())
            .take(256)
            .collect();
        anyhow::bail!("{} 以 {} 退出：{line}", cli.display(), output.status);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::resolve_install_dir;

    #[test]
    fn stopped_roles_require_an_explicit_install_dir() {
        assert!(resolve_install_dir(None, &[("worker", 0), ("admin", 0)]).is_err());
        assert_eq!(
            resolve_install_dir(Some(Path::new("/opt/auction/bin")), &[("worker", 0)])
                .expect("显式安装目录可用"),
            Path::new("/opt/auction/bin")
        );
    }
}
