//! 凭据页面的安全标签映射辅助。

use auction_web_shared::t;

use crate::surface::admin_world_label;

pub fn credential_kind_label(kind: &str) -> String {
    match kind {
        "mail_cookie" => t!("admin-credentials-kind-mail"),
        "forum_cookie" => t!("admin-credentials-kind-forum"),
        _ => kind.to_owned(),
    }
}

/// 凭据作用域除业务世界外还有跨世界共用的 `shared`；业务世界名统一委托 [`admin_world_label`]。
pub fn credential_world_label(world: &str) -> String {
    match world {
        "shared" => t!("admin-credentials-world-shared"),
        _ => admin_world_label(world),
    }
}

pub fn credential_status_label(status: &str) -> String {
    match status {
        "active" => t!("admin-credentials-status-active"),
        "disabled" => t!("admin-credentials-status-disabled"),
        "invalid" => t!("admin-credentials-status-invalid"),
        _ => status.to_owned(),
    }
}

pub fn credential_status_class(status: &str) -> &'static str {
    match status {
        "active" => "admin-credentials__status admin-credentials__status--active",
        "disabled" => "admin-credentials__status admin-credentials__status--disabled",
        "invalid" => "admin-credentials__status admin-credentials__status--invalid",
        _ => "admin-credentials__status",
    }
}
