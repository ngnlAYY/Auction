//! 装备反超 PM 传输端口；准备只读，sending fence 后只发送一次，未知结果只读对账。

use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

/// 不泄露远端正文或凭据的异步结果。
pub type ForumPmFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ForumPmError>> + Send + 'a>>;

/// 竞价事务冻结的完整 PM 内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForumPmMessage {
    pub notification_id: uuid::Uuid,
    pub recipient_forum_account_id: String,
    pub title: String,
    pub body: String,
}

/// 同一 sender 的 Sent 副本精确核验结果，而非 HTTP 成功状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumPmReceipt {
    pub message: ForumPmMessage,
    pub sender_forum_account_id: String,
    pub sent_message_id: String,
}

/// 安全错误闭集；不携带 HTML、Cookie 或令牌。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ForumPmError {
    #[error("论坛暂不可用")]
    Unavailable,
    #[error("论坛身份不可用")]
    Authentication,
    #[error("论坛请求受限")]
    RateLimited,
    #[error("论坛收件人无效")]
    InvalidRecipient,
    #[error("论坛协议无法核验")]
    Protocol,
    #[error("论坛发送结果未知，仅允许只读核对")]
    OutcomeUnknown,
}

/// 内存内绑定同一凭据会话和新表单；不得持久化凭据或实现未脱敏 Debug。
pub trait PreparedForumPm: Send {
    fn sender_forum_account_id(&self) -> &str;
    /// 仅在持久 sending fence 确认提交后消费一次，不重试 POST。
    fn send(self: Box<Self>) -> ForumPmFuture<'static, ForumPmReceipt>;
}

/// 共享论坛网关及凭据 owner；核对不到副本不代表未发送。
pub trait ForumPmSender: crate::ForumCredentialAvailability {
    /// 仅 GET/解析，无远端写入；world 是通知来源 lot 的权威世界。
    fn prepare<'a>(
        &'a self,
        world: auction_domain::SettlementWorld,
        message: &'a ForumPmMessage,
    ) -> ForumPmFuture<'a, Box<dyn PreparedForumPm>>;
    /// 仅原 sender 的只读 Sent 核对，不授权重新发送。
    fn reconcile<'a>(
        &'a self,
        world: auction_domain::SettlementWorld,
        message: &'a ForumPmMessage,
        sender_forum_account_id: &'a str,
    ) -> ForumPmFuture<'a, Option<ForumPmReceipt>>;
}
