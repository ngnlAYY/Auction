use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    common::{MailProcessingError, valid_text},
    message::MailMessage,
};
use std::{future::Future, pin::Pin};

/// 业务处理器返回的安全入站决定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailProcessingDecision {
    pub mail_kind: String,
    pub status: String,
    pub decision_code: String,
    pub metadata: Value,
    /// 首次观察到的诊断旁路数据；不进入决定序列化、业务摘要或幂等账本。
    #[serde(skip)]
    pub equipment_diagnostic: Option<crate::MailEquipmentDiagnosticSnapshot>,
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

pub trait MailProcessor: Send + Sync {
    fn process(&self, message: MailMessage) -> MailProcessingFuture<'_>;
}

/// 单次业务尝试及其可选留证任务；业务错误仍按原有类型和重试规则处理。
pub struct MailProcessingAttempt {
    pub result: Result<MailProcessingDecision, crate::ApplicationError>,
    pub pending_equipment_diagnostic: Option<crate::PendingMailEquipmentDiagnostic>,
}

pub type MailProcessingFuture<'a> =
    Pin<Box<dyn Future<Output = MailProcessingAttempt> + Send + 'a>>;
