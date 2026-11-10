//! WAL 归档、每周基础备份与暂存上传。`archive` 是 PostgreSQL `archive_command`
//! 的入口，只写本地暂存；`base-backup` 生成物理基础备份并写入同一暂存目录；`upload` 是独立
//! systemd timer 的入口，只读暂存并写 R2。三者共享暂存目录与 manifest 约定。

use crate::{app::print_json, cli::WalCommand, deployment::ConfigDir};

pub(crate) use evidence::staged_wal_facts;

mod archive;
mod base;
mod evidence;
mod model;
mod upload;

pub(crate) async fn run(config_dir: &ConfigDir, command: WalCommand) -> anyhow::Result<()> {
    match command {
        WalCommand::Archive { staging_dir, recipient, source, name } => {
            let outcome =
                archive::run(&archive::Request { staging_dir, recipient, source, name }).await?;
            print_json(&outcome)
        }
        WalCommand::BaseBackup { staging_dir, recipient } => {
            let outcome = base::run(&base::Request { staging_dir, recipient }).await?;
            print_json(&outcome)
        }
        WalCommand::Upload { staging_dir, budget_seconds } => {
            upload::run(config_dir, &staging_dir, budget_seconds).await
        }
    }
}
