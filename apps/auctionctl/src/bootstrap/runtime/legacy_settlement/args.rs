//! 单笔 legacy 修复的命令行断言；UID、版本与摘要均不能替代真实证据。

use clap::{Args, Subcommand};
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub(crate) enum LegacySettlementAction {
    /// 预览旧自寄记录；execute 只以当前库存/身份核验后追加内部收口事实。
    SelfSend {
        #[command(flatten)]
        args: LegacyTargetArgs,
        /// 追加审计所需的人工核对原因；只有 execute 才接受。
        #[arg(long, requires = "execute")]
        reason: Option<String>,
    },
    /// 预览历史已领取的买家付款；execute 精确核对原 Sent 和 Main 现金后留存卖家款。
    AdoptCollected {
        #[command(flatten)]
        args: LegacyTargetArgs,
    },
}

#[derive(Debug, Args)]
pub(crate) struct LegacyTargetArgs {
    /// 唯一修复目标；本命令没有批量 execute 模式。
    #[arg(long)]
    pub(crate) obligation_id: Uuid,
    /// 预期论坛/HV UID；execute 仍须从同一真实会话认证读取。
    #[arg(long)]
    pub(crate) expected_uid: String,
    #[arg(long)]
    pub(crate) execute: bool,
    /// 原请求重试必须沿用同一个非零 UUID。
    #[arg(long, requires = "execute")]
    pub(crate) command_id: Option<Uuid>,
    #[arg(long, requires = "execute")]
    pub(crate) actor: Option<String>,
    /// 预览返回的履约状态版本。
    #[arg(long, requires = "execute")]
    pub(crate) expected_state_version: Option<i64>,
    /// 预览返回的 canonical SHA-256 小写十六进制。
    #[arg(long, requires = "execute")]
    pub(crate) expected_digest: Option<String>,
    /// 必须逐字使用该目标预览返回的确认短语。
    #[arg(long, requires = "execute")]
    pub(crate) confirm_write: Option<String>,
}

#[cfg(feature = "postgres")]
pub(super) struct Execution {
    pub(super) command_id: Uuid,
    pub(super) actor: String,
    pub(super) state_version: i64,
    pub(super) digest: [u8; 32],
    pub(super) confirm_write: String,
}

#[cfg(feature = "postgres")]
impl LegacyTargetArgs {
    pub(super) fn validate(&self) -> anyhow::Result<Option<Execution>> {
        use anyhow::Context;

        anyhow::ensure!(!self.obligation_id.is_nil(), "--obligation-id 必须是非零 UUID");
        anyhow::ensure!(
            auction_application::valid_external_positive_id(&self.expected_uid),
            "--expected-uid 必须是 canonical 正十进制 UID"
        );
        if !self.execute {
            return Ok(None);
        }
        let command_id = self.command_id.context("execute 要求 --command-id")?;
        anyhow::ensure!(!command_id.is_nil(), "--command-id 必须是非零 UUID");
        let actor = self.actor.clone().context("execute 要求 --actor")?;
        anyhow::ensure!(
            !actor.is_empty()
                && actor == actor.trim()
                && actor.len() <= 128
                && !actor.chars().any(char::is_control),
            "--actor 必须是长度不超过 128 字节的非空操作者标识"
        );
        let state_version =
            self.expected_state_version.context("execute 要求 --expected-state-version")?;
        anyhow::ensure!(state_version > 0, "--expected-state-version 必须为正整数");
        let digest = parse_digest(
            self.expected_digest.as_deref().context("execute 要求 --expected-digest")?,
        )?;
        let confirm_write = self.confirm_write.clone().context("execute 要求 --confirm-write")?;
        Ok(Some(Execution { command_id, actor, state_version, digest, confirm_write }))
    }
}

#[cfg(feature = "postgres")]
fn parse_digest(value: &str) -> anyhow::Result<[u8; 32]> {
    anyhow::ensure!(
        value.len() == 64
            && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "--expected-digest 必须是 64 位小写 SHA-256 十六进制"
    );
    let mut result = [0; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(result)
}
