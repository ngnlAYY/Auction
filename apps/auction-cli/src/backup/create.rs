//! 手动与定时入口共用的备份实现；只在命令外层输出 JSON，内部返回已发布的 manifest。

use std::mem::DropGuard;
use std::path::{Path, PathBuf};

use anyhow::Context;
use auction_config::AppConfig;
use auction_time::Timestamp;

use super::manifest::{FORMAT, manifest_file_name};
use super::{BackupManifest, libpq, schema_version, scope};
use crate::app::{print_json, progress};
use crate::config_dir::ConfigDir;
use crate::support::digest::{sha256_bytes, sha256_file};
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
    // 范围收集失败时 archive 保留，但不发布 range sidecar。
    let scope = scope::create(&config, output).await?;
    print_json(&serde_json::json!({
        "archive": output,
        "manifest": manifest_path,
        "archive_sha256": manifest.archive_sha256,
        "schema_version": manifest.schema_version,
        "scope": scope,
    }))
}

/// 只消费冻结的配置快照；保护 schema 前后版本、无覆盖发布与配对 manifest 的不变量。
pub(crate) async fn create_backup(
    config: &AppConfig,
    database: &str,
    output: &Path,
) -> anyhow::Result<BackupManifest> {
    let (archive_name, manifest_path) = targets(output)?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    let environment = libpq::Libpq::from_url(
        url,
        database,
        config.database_profiles.mode,
        auction_storage::DatabaseConnectionPurpose::CliBackup,
    )?;
    crate::database::with_store(
        config,
        auction_storage::DatabaseConnectionPurpose::CliBackupCheck,
        async |store| {
            let before = schema_version(store, database).await?;
            // 总预算取消也回收本次临时名，不删除已发布的备份。
            let temporary = DropGuard::new(
                output.with_file_name(format!(".{archive_name}.{}.partial", uuid::Uuid::new_v4())),
                |path| {
                    let _ = std::fs::remove_file(path);
                },
            );
            progress(format_args!("pg_dump {database}（schema {before}）"));
            environment.dump(&temporary).await?;
            let archive = files::open_regular(&temporary)?;
            libpq::list_archive(&archive).await?;
            let after = schema_version(store, database).await?;
            if after != before {
                anyhow::bail!("备份期间 schema 版本从 {before} 变为 {after}，archive 已丢弃");
            }
            archive.sync_all().context("同步 archive 失败")?;
            let manifest = BackupManifest {
                format: FORMAT.to_owned(),
                archive_file_name: archive_name.to_owned(),
                database_name_sha256: sha256_bytes(database.as_bytes()),
                archive_sha256: sha256_file(&temporary).await?,
                schema_version: before,
                created_at: Timestamp::now(),
            };
            let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
            let staged_manifest = manifest_path.with_file_name(format!(
                ".{}.{}.partial",
                archive_name,
                uuid::Uuid::new_v4()
            ));
            let staged_manifest_guard = DropGuard::new(staged_manifest.clone(), |path| {
                let _ = std::fs::remove_file(path);
            });
            files::write_private(&staged_manifest, &manifest_bytes)?;
            // 两个最终名称都使用硬链接无覆盖发布；任一后续错误都会回收未配对 archive。
            let mut archive_published = false;
            let mut manifest_published = false;
            let publish_result = (|| -> anyhow::Result<()> {
                std::fs::hard_link(temporary.as_path(), output)
                    .with_context(|| format!("发布 archive 失败：{}", output.display()))?;
                archive_published = true;
                // 先让 archive 目录项落盘再发布 manifest：崩溃后最多留下没有 manifest 的
                // archive（可重新备份），不会出现引用不存在 archive 的 manifest。
                files::sync_parent(output)?;
                std::fs::hard_link(&staged_manifest, &manifest_path).with_context(|| {
                    format!("发布 archive manifest 失败：{}", manifest_path.display())
                })?;
                manifest_published = true;
                files::sync_parent(&manifest_path)?;
                Ok(())
            })();
            if let Err(error) = publish_result {
                let cleanup = (|| -> anyhow::Result<()> {
                    if manifest_published {
                        std::fs::remove_file(&manifest_path)
                            .context("清理未配对的 archive manifest 失败")?;
                    }
                    if archive_published {
                        std::fs::remove_file(output).context("清理未配对的 archive 失败")?;
                    }
                    if archive_published || manifest_published {
                        files::sync_parent(output).context("同步未配对备份清理失败")?;
                    }
                    Ok(())
                })();
                return Err(match cleanup {
                    Ok(()) => error,
                    Err(cleanup_error) => {
                        error.context(format!("另有发布文件清理失败：{cleanup_error:#}"))
                    }
                });
            }
            drop(staged_manifest_guard);
            drop(temporary);
            Ok(manifest)
        },
    )
    .await
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
