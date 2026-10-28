use chrono::{DateTime, Utc};
use uuid::Uuid;

use auction_domain::CommandId;

use super::super::{ApplicationFuture, RequestHash};

/// Delivery 操作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDeliveryOperation {
    Send,
    Return,
    Take,
}

impl MailDeliveryOperation {
    /// 返回稳定的 outbox 操作名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Return => "return",
            Self::Take => "take",
        }
    }
}

/// MoogleMail delivery 的业务 owner 闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailDeliveryOrigin {
    MailIntake,
    SettlementRecoveryAction,
    OperationsAlert,
    LotWatch,
}

impl MailDeliveryOrigin {
    /// 返回 deliveries.origin_kind 使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MailIntake => "mail_intake",
            Self::SettlementRecoveryAction => "settlement_recovery_action",
            Self::OperationsAlert => "operations_alert",
            Self::LotWatch => "lot_watch",
        }
    }
}

/// MoogleMail 通知模板闭集；正文不能由入站邮件任意注入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailNotificationTemplate {
    AuctionAssetsRejected,
    AuctionNoAttachments,
    AuctionEquipmentRejected,
    UserAssetsRejected,
    RegisterKey,
    RegisterResult,
    RebindResult,
    ForumAuctionNotice,
    RecoveryPaymentReminder,
    OperationsAlert,
    LotWatch,
}

impl MailNotificationTemplate {
    /// 返回稳定的模板名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuctionAssetsRejected => "auction_assets_rejected",
            Self::AuctionNoAttachments => "auction_no_attachments",
            Self::AuctionEquipmentRejected => "auction_equipment_rejected",
            Self::UserAssetsRejected => "user_assets_rejected",
            Self::RegisterKey => "register_key",
            Self::RegisterResult => "register_result",
            Self::RebindResult => "rebind_result",
            Self::ForumAuctionNotice => "forum_auction_notice",
            Self::RecoveryPaymentReminder => "recovery_payment_reminder",
            Self::OperationsAlert => "operations_alert",
            Self::LotWatch => "lot_watch",
        }
    }
}

/// 由业务 owner 生成的无密文 MoogleMail delivery 命令。
#[derive(Debug, Clone)]
pub struct MailDeliveryCommand {
    pub command_id: CommandId,
    pub request_hash: RequestHash,
    pub origin: MailDeliveryOrigin,
    pub origin_id: String,
    pub operation: MailDeliveryOperation,
    pub world_id: String,
    pub external_mail_id: String,
    pub recipient: String,
    pub template: Option<MailNotificationTemplate>,
    pub result_code: Option<String>,
    /// 注册 key 通知使用的用户 ID 引用；只允许和 register_key 模板一起出现。
    pub user_key_user_id: Option<Uuid>,
    pub credits: i64,
    pub cod: i64,
    pub attachment_ids: Vec<String>,
    /// operations alert 或 lot watch 使用的已冻结安全摘要；其他 delivery 必须省略。
    pub notification_body: Option<String>,
}

/// 已冻结通知摘要（operations alert、lot watch）的格式：非空、无首尾空白、不超过 16 KiB，
/// 控制字符只允许换行与制表；入队、存储重放核对与发送前校验共用同一规则。
pub fn valid_notification_body(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        !value.is_empty()
            && value.len() <= (16 << 10)
            && value == value.trim()
            && !value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    })
}

/// MoogleMail delivery 入队后的安全结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailDeliveryResult {
    pub delivery_id: Uuid,
}

/// recovery delivery 激活时使用的稳定身份。
/// prepare 属于 delivery 行本身的幂等身份；activate 属于 recovery action 的第二个本地事务
/// 边界。两者分开保存，才能在 delivery 已经进入 pending 而 action 尚未写回时安全重放。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailDeliveryActivation {
    pub delivery_id: Uuid,
    pub prepare_command_id: CommandId,
    pub prepare_request_hash: RequestHash,
    pub activate_command_id: CommandId,
    pub activate_request_hash: RequestHash,
    pub activated_at: DateTime<Utc>,
}

/// MoogleMail delivery outbox 入队端口。
pub trait MailDeliveryEnqueuer: Send + Sync {
    fn enqueue(&self, command: MailDeliveryCommand) -> ApplicationFuture<'_, MailDeliveryResult>;

    /// 只读恢复旧版已准备但尚未关联 action 的同邮件投递；完整负载不符必须失败关闭。
    /// 返回 None 只表示该旧命令尚不存在，或已绑定另一次退件，调用者才能准备新轮次。
    /// 未装配查询能力的实现明确拒绝，不能把未知当作原投递不存在。
    fn find_recovery_preparation(
        &self,
        _command: MailDeliveryCommand,
    ) -> ApplicationFuture<'_, Option<MailDeliveryResult>> {
        Box::pin(async {
            Err(crate::ApplicationError::CapabilityUnavailable("recovery preparation lookup"))
        })
    }

    /// 以 prepared 状态写入 recovery delivery；普通通知仍直接进入 pending。
    fn prepare_recovery(
        &self,
        command: MailDeliveryCommand,
    ) -> ApplicationFuture<'_, MailDeliveryResult>;

    /// 将已校验的 recovery delivery 从 prepared 激活为可领取的 pending。
    fn activate_recovery(
        &self,
        activation: MailDeliveryActivation,
    ) -> ApplicationFuture<'_, MailDeliveryResult>;
}
