//! 在线开关命令；稳定身份、版本与原因均由调用方明确提供。

use clap::{Args, Subcommand};
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub(crate) enum SwitchCommand {
    /// 读取数据库最新意图；不表示运行进程已经全部应用。
    List,
    /// 设置覆盖；绝对期限与无期限必须二选一。
    Set {
        /// 开关名称，如 external_access_paused；闭集见 `switch list`，未知名称拒绝。
        name: String,
        /// 覆盖值：readiness_audit_failure_mode 与 disposition_read_mode 取 legacy、shadow 或
        /// classified，其余开关只接受 true 或 false；metrics_reject_forwarded_requests 只允许
        /// 在线收紧，set false 被拒绝。
        value: String,
        #[command(flatten)]
        write: SwitchWriteArgs,
        /// 覆盖的绝对到期时间（RFC3339，如 2026-12-01T00:00:00Z，最多六位小数），到期后回落
        /// TOML 基线；与 --no-expiry 二选一，必须提供其一。
        #[arg(
            long,
            value_name = "RFC3339",
            required_unless_present = "no_expiry",
            conflicts_with = "no_expiry"
        )]
        expires_at: Option<String>,
        /// 不设期限：覆盖跨重启保留，直到 clear；与 --expires-at 二选一。
        #[arg(long)]
        no_expiry: bool,
    },
    /// 追加清除事实，回落各角色启动时的 TOML 基线。
    Clear {
        /// 开关名称；闭集见 `switch list`。
        name: String,
        #[command(flatten)]
        write: SwitchWriteArgs,
    },
}

#[derive(Debug, Args)]
pub(crate) struct SwitchWriteArgs {
    /// 本次写入的稳定非零 UUID：结果未知时用同一 ID 与全部原参数原样重试，新的意图换新 ID。
    #[arg(long)]
    pub(crate) command_id: Uuid,
    /// 期望的当前版本，取 `switch list` 中该开关的 revision（从未设置为 0）；不一致即拒绝。
    #[arg(long, value_parser = clap::value_parser!(i64).range(0..))]
    pub(crate) expected_revision: i64,
    /// 操作者说明：1–128 字节、首尾无空白且不含控制字符，写入审计。
    #[arg(long)]
    pub(crate) actor: String,
    /// 变更原因：1–1024 字节、首尾无空白且不含控制字符，写入审计；不要填写秘密或玩家信息。
    #[arg(long)]
    pub(crate) reason: String,
}
