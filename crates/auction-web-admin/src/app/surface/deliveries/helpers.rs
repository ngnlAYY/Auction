//! Delivery 状态和来源辅助。

use auction_web_shared::legacy_t as t;

use crate::api::Delivery;

pub(crate) const DELIVERY_STATUS_VALUES: [&str; 7] =
    ["prepared", "pending", "claimed", "sending", "succeeded", "failed", "manual_reconcile"];

/// 历史资产处置冲突即使已人工标为失败也禁止重发。
pub(super) fn is_mail_disposition_conflict(delivery: &Delivery) -> bool {
    delivery.reason_code.as_deref() == Some("mail_disposition_conflict")
}

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

    numeric_suffix.map(str::to_owned).unwrap_or_else(|| delivery.id.chars().take(8).collect())
}

/// 旧页面把 settlement 显示为 equipment；新版合法扩展使用五语言安全标签，避免泄漏内部 token。
pub(super) fn delivery_display_type(origin_kind: &str) -> String {
    match origin_kind {
        "settlement" => "equipment".to_owned(),
        "settlement_recovery_action" => t!("delivery-type-settlement-recovery"),
        "operations_alert" => t!("delivery-type-operations-alert"),
        "lot_watch" => t!("delivery-type-lot-watch"),
        "seller_summary" => t!("delivery-type-seller-summary"),
        "season_reminder" => t!("delivery-type-season-reminder"),
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
