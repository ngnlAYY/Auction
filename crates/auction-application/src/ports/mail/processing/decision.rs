//! 入站邮件的处理决定与处理器端口。

use std::{future::Future, pin::Pin};

use auction_domain::valid_text;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{errors::MailProcessingError, message::MailMessage};

/// 业务处理器返回的安全入站决定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailProcessingDecision {
    pub mail_kind: String,
    pub status: String,
    pub decision_code: String,
    pub metadata: Value,
    /// 首次观察到的诊断旁路数据；不进入决定序列化、业务摘要或幂等账本。
    #[serde(skip)]
    pub equipment_diagnostic: Option<crate::ports::mail::MailEquipmentDiagnosticSnapshot>,
}

impl MailProcessingDecision {
    /// 校验决定只包含数据库允许的闭集和 JSON 对象。
    pub fn validate(&self) -> Result<(), MailProcessingError> {
        if !matches!(
            self.mail_kind.as_str(),
            "equipment" | "bid" | "delivery" | "registration" | "rebind" | "rekey" | "unknown"
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

    /// 在不改变旧路由优先级的前提下给或许可器分流。
    ///
    /// 默认实现只调用原处理器；实现寄售候选分流的处理器必须覆盖本方法，先完成世界校验、
    /// 既有接受续办、精确主题与系统结算退件识别，只有全部无法归属且调用方确认该信带寄售
    /// 候选时才返回 [`MailProcessingRoute::ConsignmentCandidate`]。
    fn route(&self, message: MailMessage, _consignment_candidate: bool) -> MailRoutingFuture<'_> {
        Box::pin(async move { MailProcessingRoute::Legacy(Box::new(self.process(message).await)) })
    }
}

/// 单次业务尝试及其可选留证任务；业务错误仍按原有类型和重试规则处理。
pub struct MailProcessingAttempt {
    pub result: Result<MailProcessingDecision, crate::errors::ApplicationError>,
    pub pending_equipment_diagnostic: Option<crate::ports::mail::PendingMailEquipmentDiagnostic>,
}

pub type MailProcessingFuture<'a> =
    Pin<Box<dyn Future<Output = MailProcessingAttempt> + Send + 'a>>;

/// 旧路由决定或寄售候选的两态结果。
pub enum MailProcessingRoute {
    /// 旧路由已归属或已按未知邮件完成处理。
    Legacy(Box<MailProcessingAttempt>),
    /// 旧路由全部无法归属，且该信确实带 4D1 寄售候选。
    ConsignmentCandidate,
}

/// 一次路由尝试；候选不含旧处理器的留证任务。
pub type MailRoutingFuture<'a> = Pin<Box<dyn Future<Output = MailProcessingRoute> + Send + 'a>>;
