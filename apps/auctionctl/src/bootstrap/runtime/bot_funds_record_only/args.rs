//! 只记账开户的命令行断言；稳定身份、版本及摘要与原命令一同保留。

use clap::Args;
use uuid::Uuid;

/// 默认只读预览；执行不接受余额、资金用途额度或专用账号声明。
#[derive(Debug, Args)]
pub(crate) struct BotFundsRecordOnlyArgs {
    /// 预期论坛/HV UID；执行仍须从真实会话认证读取。
    #[arg(long)]
    pub(super) expected_uid: String,
    #[arg(long)]
    pub(super) execute: bool,
    /// 结果未知时必须沿用原非零 UUID。
    #[arg(long, requires = "execute")]
    pub(super) command_id: Option<Uuid>,
    #[arg(long, requires = "execute")]
    pub(super) actor: Option<String>,
    /// 缺失账户的预期版本为零。
    #[arg(long, requires = "execute")]
    pub(super) expected_version: Option<i64>,
    #[arg(long, requires = "execute")]
    pub(super) preview_digest: Option<String>,
    /// 逐字使用预览返回的确认短语。
    #[arg(long, requires = "execute")]
    pub(super) confirm_write: Option<String>,
}

#[cfg(feature = "postgres")]
/// 已通过命令行形状校验的显式执行参数；业务授权仍由应用命令校验。
pub(super) struct Execution {
    pub(super) command_id: Uuid,
    pub(super) actor: String,
    pub(super) expected_version: i64,
    pub(super) preview_digest: [u8; 32],
    pub(super) confirm_write: String,
}

#[cfg(feature = "postgres")]
impl BotFundsRecordOnlyArgs {
    /// 预览只校验 UID；执行要求保留完整稳定参数，不能填入余额或用途额度。
    pub(super) fn validate(&self) -> anyhow::Result<Option<Execution>> {
        use anyhow::Context;

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
        let expected_version = self.expected_version.context("execute 要求 --expected-version")?;
        anyhow::ensure!(expected_version >= 0, "--expected-version 必须为非负整数");
        let value = self.preview_digest.as_deref().context("execute 要求 --preview-digest")?;
        anyhow::ensure!(
            value.len() == 64
                && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "--preview-digest 必须是 64 位小写 SHA-256 十六进制"
        );
        let mut preview_digest = [0; 32];
        for (index, byte) in preview_digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
        }
        let confirm_write = self.confirm_write.clone().context("execute 要求 --confirm-write")?;
        Ok(Some(Execution { command_id, actor, expected_version, preview_digest, confirm_write }))
    }
}
