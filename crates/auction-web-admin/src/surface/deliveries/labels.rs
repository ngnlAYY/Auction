//! Delivery 状态和来源辅助。

use auction_contracts::AdminDeliveryActionBlocker;
use auction_web_shared::t;

use crate::api::{Delivery, SentProofCommand};
use crate::surface::AdminActionFeedback;

pub(crate) fn action_blocker_label(reason: AdminDeliveryActionBlocker) -> String {
    use AdminDeliveryActionBlocker as Blocker;

    match reason {
        Blocker::SettlementReturnActionOwned => t!("delivery-action-blocked-return-owned"),
        Blocker::SettlementIneligible => t!("delivery-action-blocked-settlement"),
        Blocker::FundedDelivery => t!("delivery-action-blocked-funded"),
        Blocker::RetiredDelivery => t!("delivery-action-blocked-retired"),
        Blocker::StaleFencingVersion => t!("delivery-action-blocked-stale"),
        Blocker::SentSighted => t!("delivery-action-blocked-sent-sighted"),
        Blocker::ReconcileSucceededForSend => t!("delivery-reconcile-send-proof-required"),
        Blocker::DeliveryNotEligible => t!("delivery-action-blocked-ineligible"),
        Blocker::StatusMismatch => t!("delivery-action-blocked-status"),
        Blocker::QuarantinedMailActionNotAllowed => t!("delivery-mail-disposition-conflict"),
        Blocker::MissingReconcileReference => t!("delivery-action-blocked-reference"),
        Blocker::RecipientRenamed => t!("delivery-action-blocked-recipient-renamed"),
        Blocker::AlreadyDisposed => t!("delivery-action-blocked-disposed"),
        Blocker::NotAcknowledgeable => t!("delivery-action-blocked-acknowledge"),
        Blocker::RecoveryRetryIneligible => t!("delivery-action-blocked-recovery-retry"),
        Blocker::SettlementFulfillmentState => t!("delivery-action-blocked-fulfillment-state"),
        Blocker::MissingSendingFence => t!("delivery-action-blocked-missing-sending-fence"),
        Blocker::DeliveryCommandSnapshotUnreadable => {
            t!("delivery-action-blocked-command-snapshot-unreadable")
        }
    }
}

pub(crate) fn action_feedback_message(feedback: &AdminActionFeedback, success: String) -> String {
    match feedback {
        AdminActionFeedback::Error(code) if code == "delivery_recipient_renamed" => {
            action_blocker_label(AdminDeliveryActionBlocker::RecipientRenamed)
        }
        AdminActionFeedback::Error(code) if code == "delivery_command_snapshot_unreadable" => {
            action_blocker_label(AdminDeliveryActionBlocker::DeliveryCommandSnapshotUnreadable)
        }
        _ => feedback.message(success),
    }
}

/// 把接口校验拒绝的理由草稿映射为字段提示。
pub(super) fn delivery_reason_error(reason: &str) -> Option<String> {
    let reason = reason.trim();
    if SentProofCommand::valid_reason(reason) {
        return None;
    }
    let max = SentProofCommand::MAX_REASON_BYTES;
    Some(if reason.is_empty() {
        t!("admin-field-required")
    } else if reason.len() > max {
        t!("admin-field-max-utf8-bytes", max: max)
    } else {
        t!("admin-field-no-control-characters")
    })
}

pub(crate) const DELIVERY_STATUS_VALUES: [&str; 7] =
    ["prepared", "pending", "claimed", "sending", "succeeded", "failed", "manual_reconcile"];

pub(crate) fn delivery_status_label(status: &str) -> String {
    match status {
        "prepared" => t!("delivery-status-prepared"),
        "pending" => t!("delivery-status-pending"),
        "claimed" => t!("delivery-status-claimed"),
        "sending" => t!("delivery-status-sending"),
        "succeeded" => t!("delivery-status-succeeded"),
        "failed" => t!("delivery-status-failed"),
        "manual_reconcile" => t!("delivery-status-manual"),
        _ => status.to_owned(),
    }
}

/// 恢复旧 Admin 表格的短 ID：优先展示来源末尾的数字，否则回退 delivery UUID 前八位。
pub(super) fn delivery_display_id(delivery: &Delivery) -> String {
    let origin_id = delivery.origin_id.as_str();
    let numeric_suffix =
        if !origin_id.is_empty() && origin_id.as_bytes().iter().all(u8::is_ascii_digit) {
            Some(origin_id)
        } else {
            origin_id.rsplit_once(['-', '_']).map(|(_, suffix)| suffix).filter(|suffix| {
                !suffix.is_empty() && suffix.as_bytes().iter().all(u8::is_ascii_digit)
            })
        };

    numeric_suffix.map_or_else(|| delivery.id.chars().take(8).collect(), str::to_owned)
}

/// 旧页面把 settlement 显示为 equipment；新版合法扩展使用五语言安全标签，避免泄漏内部 token。
pub(super) fn delivery_display_type(origin_kind: &str) -> String {
    match origin_kind {
        "settlement" => "equipment".to_owned(),
        "settlement_recovery_action" => t!("delivery-type-settlement-recovery"),
        "settlement_return_action" => t!("delivery-type-settlement-return"),
        "operations_alert" => t!("delivery-type-operations-alert"),
        "lot_watch" => t!("delivery-type-lot-watch"),
        "seller_summary" => t!("delivery-type-seller-summary"),
        "season_reminder" => t!("delivery-type-season-reminder"),
        "consignment_notice" => t!("delivery-type-consignment-notice"),
        "consignment_disposition" => t!("delivery-type-consignment-disposition"),
        _ => origin_kind.to_owned(),
    }
}

/// 发送类别标签；未知类别返回原文，闭集由服务端保证。
pub(crate) fn delivery_category_label(category: &str) -> String {
    match category {
        "outbid" => t!("delivery-category-outbid"),
        "account" => t!("delivery-category-account"),
        "intake" => t!("delivery-category-intake"),
        "settlement" => t!("delivery-category-settlement"),
        "equipment" => t!("delivery-category-equipment"),
        "operations" => t!("delivery-category-operations"),
        "manual" => t!("delivery-category-manual"),
        _ => category.to_owned(),
    }
}

/// 行内终态说明：退役/抑制原因优先于普通失败码，二者都只在确定未写入时出现。
pub(crate) fn delivery_outcome_label(delivery: &Delivery) -> String {
    if let Some(reason) = delivery.retirement_reason.as_deref() {
        return t!("delivery-outcome-retired", reason: reason);
    }
    if let Some(reason) = delivery.suppression_reason.as_deref() {
        return t!("delivery-outcome-suppressed", reason: reason);
    }
    delivery.failure_code.clone().unwrap_or_default()
}
