//! 在线开关命令；稳定身份、版本与原因均由调用方明确提供。

use clap::{Args, Subcommand};
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub(crate) enum SwitchCommand {
    /// 读取数据库最新意图；不表示运行进程已经全部应用。
    List,
    /// 设置覆盖；绝对期限与无期限必须二选一。
    Set {
        name: String,
        value: String,
        #[command(flatten)]
        write: SwitchWriteArgs,
        #[arg(long, required_unless_present = "no_expiry", conflicts_with = "no_expiry")]
        expires_at: Option<String>,
        #[arg(long)]
        no_expiry: bool,
    },
    /// 追加清除事实，回落各角色启动时的 TOML 基线。
    Clear {
        name: String,
        #[command(flatten)]
        write: SwitchWriteArgs,
    },
}

#[derive(Debug, Args)]
pub(crate) struct SwitchWriteArgs {
    #[arg(long)]
    pub(crate) command_id: Uuid,
    #[arg(long, value_parser = clap::value_parser!(i64).range(0..))]
    pub(crate) expected_revision: i64,
    #[arg(long)]
    pub(crate) actor: String,
    #[arg(long)]
    pub(crate) reason: String,
}
