//! Delivery 状态、来源和 CSS 类辅助。

use dioxus_i18n::t;

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

pub(super) fn delivery_origin_label(origin_kind: &str) -> String {
    match origin_kind {
        "settlement" => t!("delivery-origin-settlement"),
        "settlement_recovery_action" => t!("delivery-origin-recovery"),
        "user_key" => t!("delivery-origin-user-key"),
        "manual" => t!("delivery-origin-manual"),
        "mail_intake" => t!("delivery-origin-mail-intake"),
        "operations_alert" => t!("delivery-origin-operations-alert"),
        _ => origin_kind.to_owned(),
    }
}

pub(super) fn delivery_status_class(status: &str) -> &'static str {
    match status {
        "succeeded" => "admin-delivery__status admin-delivery__status--succeeded",
        "failed" => "admin-delivery__status admin-delivery__status--failed",
        "manual_reconcile" => "admin-delivery__status admin-delivery__status--manual",
        "sending" | "claimed" => "admin-delivery__status admin-delivery__status--active",
        "prepared" | "pending" => "admin-delivery__status admin-delivery__status--pending",
        _ => "admin-delivery__status",
    }
}
