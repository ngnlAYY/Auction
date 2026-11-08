//! W9.11 账号回信与说明信预算的读取端口。
//!
//! 预算只决定通知是否入队，不参与金额、资格、幂等或审计；Worker 的完整热快照通过本端口
//! 暴露给邮件处理器，避免处理器再持有一份可独立写入的预算状态。

use std::fmt::Debug;

/// 同一账号注册/轮换回信与说明信的当前限制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailReplyPolicy {
    /// 同一账号注册/轮换回信的窗口秒数。
    pub account_reply_window_seconds: u64,
    /// 说明信窗口秒数。
    pub explanation_mail_window_seconds: u64,
    /// 说明信窗口内最大封数；0 表示不限。
    pub explanation_mail_limit: u32,
}

impl MailReplyPolicy {
    /// 与启动配置和 runtime 校验保持一致。
    pub fn is_valid(self) -> bool {
        (60..=604_800).contains(&self.account_reply_window_seconds)
            && (60..=604_800).contains(&self.explanation_mail_window_seconds)
            && self.explanation_mail_limit <= 100
    }
}

impl Default for MailReplyPolicy {
    fn default() -> Self {
        Self {
            account_reply_window_seconds: 86_400,
            explanation_mail_window_seconds: 86_400,
            explanation_mail_limit: 10,
        }
    }
}

/// 只读预算来源；实现方必须返回一份完整、已校验的快照。
pub trait MailReplyPolicySource: Debug + Send + Sync {
    fn current(&self) -> MailReplyPolicy;
}
