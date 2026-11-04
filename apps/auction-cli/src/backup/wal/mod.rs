//! WAL 归档与暂存上传（W10.7）。`archive` 是 PostgreSQL `archive_command` 的入口，只写本地
//! 暂存；`upload` 是独立 systemd timer 的入口，只读暂存并写 R2。两者共享暂存目录与 manifest。

use crate::{app::print_json, cli::WalCommand, deployment::ConfigDir};

mod archive;
mod model;
mod upload;

pub(crate) async fn run(config_dir: &ConfigDir, command: WalCommand) -> anyhow::Result<()> {
    match command {
        WalCommand::Archive { staging_dir, recipient, source, name } => {
            let outcome =
                archive::run(&archive::Request { staging_dir, recipient, source, name }).await?;
            print_json(&outcome)
        }
        WalCommand::Upload { staging_dir, budget_seconds } => {
            upload::run(config_dir, &staging_dir, budget_seconds).await
        }
    }
}
