//! W9.11 说明信与账号回信预算的进程内热快照。
//!
//! 预算只决定通知是否入队，不参与金额、资格、幂等或审计；热读取失败时继续使用上一次
//! 已校验快照。`MailProcessorConfig` 保留启动值用于兼容既有装配，Worker 会注入共享句柄。

use std::sync::{Arc, RwLock};

/// 账号回信（注册/轮换）与说明信的当前限制。
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
    /// 与启动配置和 runtime 校验保持一致，避免未校验热快照改变业务语义。
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

/// 处理器与 Worker 热读取共享的预算句柄。
///
/// 只替换完整策略；读取方拿到的是一份 `Copy` 快照。锁中毒时沿用最后一次成功发布值，
/// 不把只影响通知节奏的故障升级为进程故障（不变量 5、9）。
#[derive(Debug, Clone)]
pub struct MailReplyPolicyHandle {
    inner: Arc<RwLock<MailReplyPolicy>>,
}

impl MailReplyPolicyHandle {
    pub fn new(initial: MailReplyPolicy) -> Self {
        Self { inner: Arc::new(RwLock::new(initial)) }
    }

    pub fn current(&self) -> MailReplyPolicy {
        *self.inner.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn replace(&self, next: MailReplyPolicy) {
        *self.inner.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }
}

#[cfg(test)]
mod tests {
    use super::{MailReplyPolicy, MailReplyPolicyHandle};

    #[test]
    fn handle_publishes_complete_policy_snapshots() {
        let handle = MailReplyPolicyHandle::new(MailReplyPolicy::default());
        let next = MailReplyPolicy {
            account_reply_window_seconds: 120,
            explanation_mail_window_seconds: 60,
            explanation_mail_limit: 0,
        };
        handle.replace(next);
        assert_eq!(handle.current(), next);
    }

    #[test]
    fn policy_validation_matches_runtime_bounds() {
        assert!(MailReplyPolicy::default().is_valid());
        assert!(
            !MailReplyPolicy { account_reply_window_seconds: 59, ..MailReplyPolicy::default() }
                .is_valid()
        );
        assert!(
            !MailReplyPolicy { explanation_mail_limit: 101, ..MailReplyPolicy::default() }
                .is_valid()
        );
    }
}
