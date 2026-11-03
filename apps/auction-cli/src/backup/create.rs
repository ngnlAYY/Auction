//! 手动与定时入口共用的备份实现；只在命令外层输出 JSON，内部返回已发布的 manifest。

use std::path::{Path, PathBuf};

use anyhow::Context;
use auction_config::AppConfig;
use chrono::Utc;

use super::manifest::{FORMAT, manifest_file_name, sha256_bytes, sha256_file};
use super::{BackupManifest, libpq, schema_version};
use crate::app::{print_json, progress};
use crate::deployment::ConfigDir;
use crate::support::files;

pub(super) async fn run(
    config_dir: &ConfigDir,
    database: &str,
    output: &Path,
) -> anyhow::Result<()> {
    // 保持手动入口先检查目标文件、再读取配置的既有校验顺序。
    let (_, manifest_path) = targets(output)?;
    let config = config_dir.load_worker()?;
    let manifest = create_backup(&config, database, output).await?;
    print_json(&serde_json::json!({
        "archive": output,
        "manifest": manifest_path,
        "archive_sha256": manifest.archive_sha256,
        "schema_version": manifest.schema_version,
    }))
}

/// 只消费冻结的配置快照；保护 schema 前后版本、无覆盖发布与配对 manifest 的不变量。
pub(super) async fn create_backup(
    config: &AppConfig,
    database: &str,
    output: &Path,
) -> anyhow::Result<BackupManifest> {
    let (archive_name, manifest_path) = targets(output)?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    let environment = libpq::Libpq::from_url(url, database)?;
    let store = crate::database::connect(url).await?;
    let result = async {
        let before = schema_version(&store, database).await?;
        let temporary = TemporaryArchive(
            output.with_file_name(format!(".{archive_name}.{}.partial", uuid::Uuid::new_v4())),
        );
        progress(format_args!("pg_dump {database}（schema {before}）"));
        environment.dump(&temporary.0).await?;
        libpq::list_archive(&temporary.0).await?;
        let after = schema_version(&store, database).await?;
        if after != before {
            anyhow::bail!("备份期间 schema 版本从 {before} 变为 {after}，archive 已丢弃");
        }
        files::open_regular(&temporary.0)?.sync_all().context("同步 archive 失败")?;
        // 硬链接发布不会覆盖同名文件；作用域退出时仅回收本次临时名。
        std::fs::hard_link(&temporary.0, output)
            .with_context(|| format!("发布 archive 失败：{}", output.display()))?;
        drop(temporary);
        files::sync_parent(output)?;
        let manifest = BackupManifest {
            format: FORMAT.to_owned(),
            archive_file_name: archive_name.to_owned(),
            database_name_sha256: sha256_bytes(database.as_bytes()),
            archive_sha256: sha256_file(output).await?,
            schema_version: before,
            created_at: Utc::now(),
        };
        files::write_private(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
        Ok(manifest)
    }
    .await;
    store.postgres_pool().close().await;
    result
}

fn targets(output: &Path) -> anyhow::Result<(&str, PathBuf)> {
    let archive_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            name.ends_with(".dump")
                && !name.starts_with('.')
                && name.trim() == *name
                && !name.contains('\\')
                && !name.chars().any(char::is_control)
        })
        .context("--output 必须是不含控制字符、反斜杠或首尾空白的 .dump 文件名")?;
    let manifest_path = output.with_file_name(manifest_file_name(archive_name));
    files::require_private_target(output)?;
    files::require_private_target(&manifest_path)?;
    Ok((archive_name, manifest_path))
}

/// 总预算取消也会经过 Drop；不删除已经发布或并非本次创建的备份文件。
struct TemporaryArchive(PathBuf);

impl Drop for TemporaryArchive {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
