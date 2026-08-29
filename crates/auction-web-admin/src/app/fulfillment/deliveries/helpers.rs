//! Delivery 状态和来源辅助。

use auction_web_shared::legacy_t as t;

use crate::api::Delivery;

pub(super) const DELIVERY_STATUS_VALUES: [&str; 7] =
    ["prepared", "pending", "claimed", "sending", "succeeded", "failed", "manual_reconcile"];

pub(super) fn delivery_status_label(status: &str) -> String {
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
        _ => origin_kind.to_owned(),
    }
}
