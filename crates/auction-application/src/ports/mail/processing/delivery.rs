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
    /// W14.3 投稿接收回执：整封投稿按原邮件合并，正文在录入事务完成后冻结。
    AuctionSubmissionReceipt,
    UserAssetsRejected,
    RegisterKey,
    RegisterResult,
    RebindResult,
    RekeyKey,
    RekeyResult,
    ForumAuctionNotice,
    RecoveryPaymentReminder,
    OperationsAlert,
    LotWatch,
}

impl MailNotificationTemplate {
    /// 全部模板的稳定顺序；通知目录测试据此核对文档与模板注册一致。
    pub const ALL: [Self; 14] = [
        Self::AuctionAssetsRejected,
        Self::AuctionNoAttachments,
        Self::AuctionEquipmentRejected,
        Self::AuctionSubmissionReceipt,
        Self::UserAssetsRejected,
        Self::RegisterKey,
        Self::RegisterResult,
        Self::RebindResult,
        Self::RekeyKey,
        Self::RekeyResult,
        Self::ForumAuctionNotice,
        Self::RecoveryPaymentReminder,
        Self::OperationsAlert,
        Self::LotWatch,
    ];

    /// 返回稳定的模板名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuctionAssetsRejected => "auction_assets_rejected",
            Self::AuctionNoAttachments => "auction_no_attachments",
            Self::AuctionEquipmentRejected => "auction_equipment_rejected",
            Self::AuctionSubmissionReceipt => "auction_submission_receipt",
            Self::UserAssetsRejected => "user_assets_rejected",
            Self::RegisterKey => "register_key",
            Self::RegisterResult => "register_result",
            Self::RebindResult => "rebind_result",
            Self::RekeyKey => "rekey_key",
            Self::RekeyResult => "rekey_result",
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

/// 说明信模板闭集（决策 63）：拒收说明、换绑结果与论坛回执共用同一发件人预算。
pub const EXPLANATION_NOTICE_TEMPLATES: &[&str] = &[
    "auction_assets_rejected",
    "auction_no_attachments",
    "auction_equipment_rejected",
    "user_assets_rejected",
    "rebind_result",
    "forum_auction_notice",
];

/// 按发件人窗口建立说明信预算；`window_seconds` 由调用方按配置校验后传入。
pub fn explanation_notice_limit(
    window_seconds: u64,
    maximum: u32,
    now: DateTime<Utc>,
) -> MailNoticeLimit {
    let seconds = i64::try_from(window_seconds).unwrap_or(604_800).clamp(60, 604_800);
    MailNoticeLimit {
        templates: EXPLANATION_NOTICE_TEMPLATES,
        since: now - chrono::Duration::seconds(seconds),
        maximum,
    }
}

/// 回信预算：在 `since` 之后该收件人已入队的 `templates` 数量达到 `maximum` 时不再入队。
/// 模板集合由应用层决定；storage 只按 payload 中冻结的模板名计数，因此不复制业务策略。
#[derive(Debug, Clone, Copy)]
pub struct MailNoticeLimit {
    pub templates: &'static [&'static str],
    pub since: DateTime<Utc>,
    /// 0 表示不限制。
    pub maximum: u32,
}

impl MailNoticeLimit {
    pub fn counts(self, template: MailNotificationTemplate) -> bool {
        self.templates.contains(&template.as_str())
    }
}

/// 受预算约束的入队结果；被抑制时不产生 delivery 行，由调用方在命令与审计中记录原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailNoticeEnqueue {
    Enqueued(MailDeliveryResult),
    Suppressed,
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

    /// 在同一 Serializable 事务内先计数再入队；达到预算时返回 [`MailNoticeEnqueue::Suppressed`]。
    /// 计数按收件人不区分 ASCII 大小写与 payload 中冻结的模板名，和入队必须同事务完成，
    /// 避免并发入站邮件在同一窗口内一起越过上限。
    fn enqueue_within_limit(
        &self,
        command: MailDeliveryCommand,
        limit: MailNoticeLimit,
    ) -> ApplicationFuture<'_, MailNoticeEnqueue>;

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
