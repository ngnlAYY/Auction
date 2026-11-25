//! 付款提醒（remind）的端口命令与回执。
//!
//! 每合同一次：到期且已证寄出、仍未付才排一封通用投递；派前再验，可能已派零补发。
//! 提醒是无秘密的通知，走 `consignment_notice` 通用投递（J7），不进专用 outbox。

use auction_domain::CommandId;
use auction_domain::consignment::recovery::ReminderDecision;
use uuid::Uuid;

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::consignment::errors::{ConsignmentError, ConsignmentFuture};
use crate::ports::core::RequestHash;
use crate::ports::mail::{
    MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin, MailNotificationTemplate,
};

/// 付款提醒的固定英文主题。
pub const CONSIGNMENT_PAYMENT_REMINDER_SUBJECT: &str = "Consignment payment reminder";

/// 通知正文结果码；与模板绑定一起冻结。
pub const CONSIGNMENT_PAYMENT_REMINDER_RESULT_CODE: &str = "payment_reminder";

/// 提醒的冻结正文：只含订单状态提示，不含装备、key、预约码或任何凭据。
///
/// 暂定文案，待维护者定稿；改文案只改这里，投递行在创建时冻结正文，已排队的提醒不受影响。
pub const CONSIGNMENT_PAYMENT_REMINDER_BODY: &str = "Your consignment purchase is still awaiting payment. Please pay the COD mail you received to complete the order.";

/// 提醒投递的 ID（同时是通用投递命令 ID）：随合同确定，同合同重放得到同一投递。
pub fn payment_reminder_delivery_id(contract_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/consignment-payment-reminder/delivery/{contract_id}").as_bytes(),
    )
}

/// 由合同生成通用 Send 投递命令（来源 `consignment_notice`，模板 `consignment_payment_reminder`）。
pub fn payment_reminder_delivery_command(
    contract_id: Uuid,
    world: &str,
    recipient: &str,
) -> Result<MailDeliveryCommand, ConsignmentError> {
    if contract_id.is_nil()
        || !matches!(world, "main" | "isekai")
        || !auction_domain::valid_text(recipient, 256)
    {
        return Err(ConsignmentError::Unavailable);
    }
    let delivery_id = payment_reminder_delivery_id(contract_id);
    let request_hash = RequestHash::from_request(
        "consignment_payment_reminder",
        delivery_id,
        &(contract_id.to_string(), world, CONSIGNMENT_PAYMENT_REMINDER_BODY),
    )
    .map_err(|error| {
        record_error(
            DiagnosticLevel::Warn,
            error.diagnostic_kind(),
            "consignment_payment_reminder",
            "hash_notification",
            DiagnosticContext::default(),
        );
        ConsignmentError::Unavailable
    })?;
    Ok(MailDeliveryCommand {
        command_id: CommandId::from_uuid(delivery_id),
        request_hash,
        origin: MailDeliveryOrigin::ConsignmentNotice,
        origin_id: format!("consignment_notice:{delivery_id}"),
        operation: MailDeliveryOperation::Send,
        world_id: world.to_owned(),
        external_mail_id: format!("consignment-notice:{delivery_id}"),
        recipient: recipient.to_owned(),
        template: Some(MailNotificationTemplate::ConsignmentPaymentReminder),
        result_code: Some(CONSIGNMENT_PAYMENT_REMINDER_RESULT_CODE.to_owned()),
        user_key_user_id: None,
        credits: 0,
        cod: 0,
        attachment_ids: Vec::new(),
        notification_body: Some(CONSIGNMENT_PAYMENT_REMINDER_BODY.to_owned()),
    })
}

/// 提醒命令：命令 ID 由合同与提醒种类派生。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemindContract {
    /// 系统命令 UUIDv5。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 成交合同。
    pub contract_id: Uuid,
}

/// 提醒的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderOutcome {
    /// 已排一封投递。
    Queued {
        /// 通用投递。
        delivery_id: Uuid,
    },
    /// 已作废：不排投递，决定即终局。
    Voided(ReminderDecision),
    /// 尚不需要提醒，没有写任何行。
    NotDue,
}

/// 提醒回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReminderReceipt {
    /// 命令 ID。
    pub command_id: CommandId,
    /// 结果。
    pub outcome: ReminderOutcome,
    /// 是否重放。
    pub replayed: bool,
}

/// 付款提醒写端。
pub trait PaymentReminderStore: Send + Sync {
    /// 到期且已证寄出才作决定并落库；已建过提醒的合同重放原结果，不再排第二封。
    fn remind(&self, command: RemindContract) -> ConsignmentFuture<'_, ReminderReceipt>;

    /// 列出已到期、已证寄出且尚无提醒决定的合同。
    fn due_reminders(&self, limit: u32) -> ConsignmentFuture<'_, Vec<Uuid>>;
}
