use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::ApplicationFuture;
use super::{
    common::{MailProcessingError, valid_text},
    message::MailMessage,
};

/// 业务处理器返回的安全入站决定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailProcessingDecision {
    pub mail_kind: String,
    pub status: String,
    pub decision_code: String,
    pub metadata: Value,
}

impl MailProcessingDecision {
    /// 校验决定只包含数据库允许的闭集和 JSON 对象。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        if !matches!(
            self.mail_kind.as_str(),
            "equipment" | "bid" | "delivery" | "registration" | "rebind" | "unknown"
        ) || !matches!(self.status.as_str(), "accepted" | "rejected" | "failed")
            || !valid_text(&self.decision_code, 256)
            || !self.metadata.is_object()
        {
            return Err(MailProcessingError::InvalidDecision);
        }
        Ok(())
    }
}

/// MoogleMail 业务处理器端口。
pub trait MailProcessor: Send + Sync {
    fn process(&self, message: MailMessage) -> ApplicationFuture<'_, MailProcessingDecision>;
}
