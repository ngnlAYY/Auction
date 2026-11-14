//! 出站通知模板的数据库修订事实、读取端口与进程内生效快照。
//!
//! 模板键闭集、内置默认正文与占位符规则由 `ports::admin::runtime_config::mail_templates`
//! 定义；本模块拥有“运营改过的正文”：每次保存或恢复默认都追加一条修订事实（同时是命令
//! 账本），每个模板键以最新一条为准，没有修订或最新修订是恢复默认时使用内置默认正文。
//! 三个角色各自周期读取并替换进程内快照。快照只决定通知正文的措辞，不决定金额、资格、
//! 幂等、锁、会话或审计；读取失败时保留上一次成功的快照。

mod commands;
mod snapshot;

pub use commands::*;
pub use snapshot::*;

use std::{future::Future, pin::Pin};

/// 模板修订 owner 的稳定错误，不携带驱动原文或正文内容。
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum MailTemplateStoreError {
    #[error("邮件模板命令无效")]
    Invalid,
    /// 模板键不在本版闭集内。
    #[error("不存在该邮件模板")]
    UnknownTemplate,
    /// 正文不满足该模板的格式或占位符规则；原因取自模板 owner 的固定消息。
    #[error("邮件模板正文无效：{0}")]
    InvalidBody(&'static str),
    /// 同 ID 异参或版本已变化。
    #[error("邮件模板命令或版本冲突；重新读取后使用新命令 ID")]
    Conflict,
    #[error("邮件模板修订事实无效；保留最后成功快照")]
    Corrupt,
    #[error("邮件模板存储不可用")]
    Unavailable,
    #[error("邮件模板提交结果未知；必须使用原命令 ID 和全部原参数原样重试")]
    OutcomeUnknown,
}

/// 窄端口的异步结果。
pub type MailTemplateFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, MailTemplateStoreError>> + Send + 'a>>;

/// 存储只拥有 SQL、锁和事务；校验、摘要、重放与版本规则由命令类型定义。
pub trait MailTemplateStore: Send + Sync {
    /// 每个模板键的最新修订；从未修订过的键不出现。本版不认识的键原样返回，由调用方忽略。
    fn latest_mail_templates(&self) -> MailTemplateFuture<'_, Vec<MailTemplateEvent>>;
    /// 同事务接收修订事实、命令结果和追加审计。
    fn write_mail_template(
        &self,
        command: MailTemplateCommand,
    ) -> MailTemplateFuture<'_, MailTemplateEvent>;
}
