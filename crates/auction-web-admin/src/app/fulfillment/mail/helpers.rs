//! Admin 邮件入站列表的状态和闭集标签映射。

use auction_web_shared::legacy_t as t;

pub(super) fn mail_status_class(status: &str) -> &'static str {
    match status {
        "accepted" => "admin-mail__status admin-mail__status--accepted",
        "failed" => "admin-mail__status admin-mail__status--failed",
        _ => "admin-mail__status admin-mail__status--pending",
    }
}

pub(super) fn mail_kind_label(kind: &str) -> String {
    match kind {
        "equipment" => t!("admin-mail-kind-equipment"),
        "bid" => t!("admin-mail-kind-bid"),
        "delivery" => t!("admin-mail-kind-delivery"),
        "registration" => t!("admin-mail-kind-registration"),
        "rebind" => t!("admin-mail-kind-rebind"),
        "unknown" => t!("admin-mail-kind-unknown"),
        _ => kind.to_owned(),
    }
}

pub(super) fn mail_status_label(status: &str) -> String {
    match status {
        "accepted" => t!("admin-mail-status-accepted"),
        "rejected" => t!("admin-mail-status-rejected"),
        "failed" => t!("admin-mail-status-failed"),
        _ => status.to_owned(),
    }
}
