//! W14.8 出站通知生命周期、失败与邮资指标端口。
//!
//! 通知种类是编译期闭集，并与 [`docs/guide/mail-flows.md`] 的通知目录一一对应。
//! 标签只来自这些枚举，绝不接受收件人、正文、lot 或邮件 MID 等动态值，避免把用户数据
//! 或高基数身份带进 Prometheus。记录失败只影响观测，不改变发送、抑制或退役结果。

use std::time::Duration;

/// 出站通知种类闭集；label 使用冻结的模板键或通知身份名。
///
/// 每个变体都必须出现在 `docs/guide/mail-flows.md` 的“通知目录”表中，
/// 由 `notification_catalog` 测试核对，新增通知时不能只加枚举而不登记。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationKind {
    RegisterKey,
    RegisterResult,
    RebindResult,
    RekeyKey,
    RekeyResult,
    AuctionAssetsRejected,
    AuctionNoAttachments,
    AuctionEquipmentRejected,
    AuctionSubmissionReceipt,
    AuctionSellerSummary,
    IsekaiSeasonReminder,
    UserAssetsRejected,
    ForumAuctionNotice,
    SettlementWinnerEquipmentCod,
    SettlementPaymentCarrier,
    SettlementPaymentReminder,
    SettlementEquipmentHandoff,
    SettlementSellerPayout,
    SettlementBuyerRefund,
    LotWatchMail,
    LotWatchPm,
    OperationsAlert,
}

impl NotificationKind {
    /// 零初始化与闭集断言使用的稳定顺序；顺序与判别值一致。
    pub const ALL: [Self; 22] = [
        Self::RegisterKey,
        Self::RegisterResult,
        Self::RebindResult,
        Self::RekeyKey,
        Self::RekeyResult,
        Self::AuctionAssetsRejected,
        Self::AuctionNoAttachments,
        Self::AuctionEquipmentRejected,
        Self::AuctionSubmissionReceipt,
        Self::AuctionSellerSummary,
        Self::IsekaiSeasonReminder,
        Self::UserAssetsRejected,
        Self::ForumAuctionNotice,
        Self::SettlementWinnerEquipmentCod,
        Self::SettlementPaymentCarrier,
        Self::SettlementPaymentReminder,
        Self::SettlementEquipmentHandoff,
        Self::SettlementSellerPayout,
        Self::SettlementBuyerRefund,
        Self::LotWatchMail,
        Self::LotWatchPm,
        Self::OperationsAlert,
    ];

    /// 稳定的指标与目录 label；同时是 MoogleMail 冻结模板键（PM / 站内身份除外）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::RegisterKey => "register_key",
            Self::RegisterResult => "register_result",
            Self::RebindResult => "rebind_result",
            Self::RekeyKey => "rekey_key",
            Self::RekeyResult => "rekey_result",
            Self::AuctionAssetsRejected => "auction_assets_rejected",
            Self::AuctionNoAttachments => "auction_no_attachments",
            Self::AuctionEquipmentRejected => "auction_equipment_rejected",
            Self::AuctionSubmissionReceipt => "auction_submission_receipt",
            Self::AuctionSellerSummary => "auction_seller_summary",
            Self::IsekaiSeasonReminder => "isekai_season_reminder",
            Self::UserAssetsRejected => "user_assets_rejected",
            Self::ForumAuctionNotice => "forum_auction_notice",
            Self::SettlementWinnerEquipmentCod => "settlement_winner_equipment_cod",
            Self::SettlementPaymentCarrier => "settlement_payment_carrier",
            Self::SettlementPaymentReminder => "settlement_payment_reminder",
            Self::SettlementEquipmentHandoff => "settlement_equipment_handoff",
            Self::SettlementSellerPayout => "settlement_seller_payout",
            Self::SettlementBuyerRefund => "settlement_buyer_refund",
            Self::LotWatchMail => "lot_watch",
            Self::LotWatchPm => "lot_watch_pm",
            Self::OperationsAlert => "operations_alert",
        }
    }

    /// 由冻结的 delivery 载荷识别通知种类；非通知动作（Return/Take）返回 `None`。
    ///
    /// 只读取 `origin_kind`、`operation` 与 `template` 三个已校验字段，不解析正文。
    pub fn from_delivery(
        origin_kind: &str,
        operation: Option<&str>,
        template: Option<&str>,
    ) -> Option<Self> {
        if operation != Some("send") {
            return None;
        }
        match (origin_kind, template) {
            ("lot_watch", Some("lot_watch")) => Some(Self::LotWatchMail),
            ("operations_alert", Some("operations_alert")) => Some(Self::OperationsAlert),
            (_, Some(template)) => Self::from_template(template),
            _ => None,
        }
    }

    /// 由 MoogleMail 冻结模板键识别通知种类。
    pub fn from_template(template: &str) -> Option<Self> {
        Some(match template {
            "register_key" => Self::RegisterKey,
            "register_result" => Self::RegisterResult,
            "rebind_result" => Self::RebindResult,
            "rekey_key" => Self::RekeyKey,
            "rekey_result" => Self::RekeyResult,
            "auction_assets_rejected" => Self::AuctionAssetsRejected,
            "auction_no_attachments" => Self::AuctionNoAttachments,
            "auction_equipment_rejected" => Self::AuctionEquipmentRejected,
            "auction_submission_receipt" => Self::AuctionSubmissionReceipt,
            "auction_seller_summary" => Self::AuctionSellerSummary,
            "isekai_season_reminder" => Self::IsekaiSeasonReminder,
            "user_assets_rejected" => Self::UserAssetsRejected,
            "forum_auction_notice" => Self::ForumAuctionNotice,
            "settlement_winner_equipment_cod" => Self::SettlementWinnerEquipmentCod,
            "settlement_payment_carrier" => Self::SettlementPaymentCarrier,
            // 恢复流程持久化的 payload 身份是 `recovery_payment_reminder`，
            // 正文模板键是 `settlement_payment_reminder`；两者是同一通知种类。
            "settlement_payment_reminder" | "recovery_payment_reminder" => {
                Self::SettlementPaymentReminder
            }
            "settlement_equipment_handoff" => Self::SettlementEquipmentHandoff,
            "settlement_seller_payout" => Self::SettlementSellerPayout,
            "settlement_buyer_refund" => Self::SettlementBuyerRefund,
            "lot_watch" => Self::LotWatchMail,
            "lot_watch_pm" => Self::LotWatchPm,
            "operations_alert" => Self::OperationsAlert,
            _ => return None,
        })
    }
}

/// 通知生命周期阶段闭集；按通知种类各自累计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationStage {
    /// 已写入 outbox 或已建立渠道事实。
    Created,
    /// 在 sending 围栏前失去资格或到期，从未开始外部写入。
    Retired,
    /// 被回信预算、渠道不可用或偏好拒绝而未创建。
    Suppressed,
    /// 外部发送取得完整证明并落库。
    Sent,
    /// 确定失败或转人工对账。
    Failed,
}

impl NotificationStage {
    pub const ALL: [Self; 5] =
        [Self::Created, Self::Retired, Self::Suppressed, Self::Sent, Self::Failed];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Retired => "retired",
            Self::Suppressed => "suppressed",
            Self::Sent => "sent",
            Self::Failed => "failed",
        }
    }
}

/// 通知失败原因闭集；`send_rejected` 单独细分邮资不足。
///
/// 只从 delivery 的冻结 `failure_code`、`reason_code` 与 `category` 元数据映射，
/// 不读取远端正文或错误字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationFailure {
    SendRejected,
    /// 远端 `cannot afford the postage`；是 `send_rejected` 的子集。
    InsufficientPostage,
    RecipientNotFound,
    RecipientIsSender,
    /// 进入外部写入后结果未知，转人工对账。
    WriteUnknown,
    ProtocolDrift,
    /// 发送前准备或交接失败，尚未开始外部写入。
    PreflightBlocked,
    /// sending 租约过期，结果未知。
    LeaseExpiredOutcomeUnknown,
    /// 本地载荷非法，确定失败。
    InvalidPayload,
    /// 其他已确认失败；不细分未知错误文本。
    Other,
}

impl NotificationFailure {
    pub const ALL: [Self; 10] = [
        Self::SendRejected,
        Self::InsufficientPostage,
        Self::RecipientNotFound,
        Self::RecipientIsSender,
        Self::WriteUnknown,
        Self::ProtocolDrift,
        Self::PreflightBlocked,
        Self::LeaseExpiredOutcomeUnknown,
        Self::InvalidPayload,
        Self::Other,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SendRejected => "send_rejected",
            Self::InsufficientPostage => "insufficient_postage",
            Self::RecipientNotFound => "recipient_not_found",
            Self::RecipientIsSender => "recipient_is_sender",
            Self::WriteUnknown => "write_unknown",
            Self::ProtocolDrift => "protocol_drift",
            Self::PreflightBlocked => "preflight_blocked",
            Self::LeaseExpiredOutcomeUnknown => "lease_expired_outcome_unknown",
            Self::InvalidPayload => "invalid_payload",
            Self::Other => "other",
        }
    }

    /// 由冻结的 delivery 失败字段映射闭集原因；`stage`/`category` 只做精确比较。
    pub fn from_delivery_failure(
        failure_code: &str,
        reason_code: Option<&str>,
        category: Option<&str>,
    ) -> Self {
        match failure_code {
            "send_rejected" | "send_rejected_after_stage" => match reason_code {
                Some("insufficient_postage") => Self::InsufficientPostage,
                _ => Self::SendRejected,
            },
            "recipient_not_found" => Self::RecipientNotFound,
            "recipient_is_sender" => Self::RecipientIsSender,
            "write_unknown" => Self::WriteUnknown,
            "preflight_unproven" => Self::ProtocolDrift,
            "preflight_timeout" | "preflight_blocked" => Self::PreflightBlocked,
            "lease_expired" | "lease_expired_outcome_unknown" => Self::LeaseExpiredOutcomeUnknown,
            "invalid_payload" => Self::InvalidPayload,
            "protocol_drift" => Self::ProtocolDrift,
            _ => match category {
                Some("protocol_drift") => Self::ProtocolDrift,
                Some("insufficient_postage") => Self::InsufficientPostage,
                Some("write_rejected") => Self::SendRejected,
                _ => Self::Other,
            },
        }
    }
}

/// 通知指标的低基数记录端口。
///
/// 所有方法都只接受闭集枚举与数值；实现不得丢弃错误，也不得把标签值写回业务结果。
/// 进程内计数是观测近似：事务重试或回滚可能让 `created` 多于实际提交行，因此不得把
/// 这些计数用于计费、审计或幂等判断；`sent` 只在终态落库成功后记录。
pub trait NotificationRecorder: Send + Sync {
    /// 记录一次生命周期阶段（创建、退役、抑制、发送、失败）。
    fn observe_stage(&self, kind: NotificationKind, stage: NotificationStage);

    /// 记录一次失败原因；调用方同时记录 [`NotificationStage::Failed`]。
    fn observe_failure(&self, kind: NotificationKind, failure: NotificationFailure);

    /// 记录从创建到终态的送达延迟；`Retired`/`Suppressed` 不应记录。
    fn observe_delivery_latency(&self, kind: NotificationKind, latency: Duration);

    /// 累加一次已发送通知的估算邮资消耗；金额未核实时由调用方传入 0。
    fn observe_postage_estimate(&self, kind: NotificationKind, credits: u64);
}

#[derive(Debug, Default)]
struct NoopNotificationRecorder;

impl NotificationRecorder for NoopNotificationRecorder {
    fn observe_stage(&self, _kind: NotificationKind, _stage: NotificationStage) {}

    fn observe_failure(&self, _kind: NotificationKind, _failure: NotificationFailure) {}

    fn observe_delivery_latency(&self, _kind: NotificationKind, _latency: Duration) {}

    fn observe_postage_estimate(&self, _kind: NotificationKind, _credits: u64) {}
}

pub fn noop_notification_recorder() -> std::sync::Arc<dyn NotificationRecorder> {
    std::sync::Arc::new(NoopNotificationRecorder)
}

// 存储适配器把记录器保存在 `Debug` 的共享状态里；记录器只暴露闭集观测，不泄漏业务数据。
impl std::fmt::Debug for dyn NotificationRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NotificationRecorder")
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::ports::notification_metrics as parent;

    #[test]
    fn kind_order_matches_discriminants_and_labels_are_unique() {
        let mut labels = std::collections::BTreeSet::new();
        for (index, kind) in NotificationKind::ALL.into_iter().enumerate() {
            assert_eq!(kind as usize, index, "NotificationKind::ALL 顺序必须与判别值一致");
            assert!(labels.insert(kind.label()), "通知种类 label 不得重复");
            assert_eq!(NotificationKind::from_template(kind.label()), Some(kind));
        }
        let mut stages = std::collections::BTreeSet::new();
        for (index, stage) in NotificationStage::ALL.into_iter().enumerate() {
            assert_eq!(stage as usize, index);
            assert!(stages.insert(stage.label()));
        }
        let mut failures = std::collections::BTreeSet::new();
        for (index, failure) in NotificationFailure::ALL.into_iter().enumerate() {
            assert_eq!(failure as usize, index);
            assert!(failures.insert(failure.label()));
        }
    }

    #[test]
    fn delivery_mapping_only_accepts_send_notifications() {
        assert_eq!(
            NotificationKind::from_delivery("lot_watch", Some("send"), Some("lot_watch")),
            Some(NotificationKind::LotWatchMail)
        );
        assert_eq!(
            NotificationKind::from_delivery(
                "operations_alert",
                Some("send"),
                Some("operations_alert")
            ),
            Some(NotificationKind::OperationsAlert)
        );
        assert_eq!(
            NotificationKind::from_delivery("mail_intake", Some("send"), Some("register_key")),
            Some(NotificationKind::RegisterKey)
        );
        // Return/Take 不是通知，未知模板也不能隐式归类。
        assert_eq!(
            NotificationKind::from_delivery("mail_intake", Some("return"), Some("register_key")),
            None
        );
        assert_eq!(
            NotificationKind::from_delivery("settlement", Some("send"), Some("unknown")),
            None
        );
        assert_eq!(NotificationKind::from_delivery("settlement", Some("send"), None), None);
    }

    #[test]
    fn failure_mapping_splits_insufficient_postage() {
        assert_eq!(
            NotificationFailure::from_delivery_failure(
                "send_rejected",
                Some("insufficient_postage"),
                Some("write_rejected")
            ),
            NotificationFailure::InsufficientPostage
        );
        assert_eq!(
            NotificationFailure::from_delivery_failure("send_rejected", None, None),
            NotificationFailure::SendRejected
        );
        assert_eq!(
            NotificationFailure::from_delivery_failure("send_rejected_after_stage", None, None),
            NotificationFailure::SendRejected
        );
        assert_eq!(
            NotificationFailure::from_delivery_failure(
                "unknown_code",
                None,
                Some("protocol_drift")
            ),
            NotificationFailure::ProtocolDrift
        );
        assert_eq!(
            NotificationFailure::from_delivery_failure("unknown_code", None, None),
            NotificationFailure::Other
        );
    }
}
