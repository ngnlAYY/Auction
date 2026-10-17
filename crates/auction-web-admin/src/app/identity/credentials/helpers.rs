//! 凭据页面的安全标签映射辅助。
//! 幂等 command_id 复用与动作执行骨架已收敛到 `app::surface`（`stable_command_id` /
//! `run_admin_feedback_action`），本模块只保留凭据专属的展示映射。

use auction_web_shared::legacy_t as t;

pub fn credential_kind_label(kind: &str) -> String {
    match kind {
        "mail_cookie" => t!("admin-credentials-kind-mail"),
        "forum_cookie" => t!("admin-credentials-kind-forum"),
        _ => kind.to_owned(),
    }
}

pub fn credential_world_label(world: &str) -> String {
    match world {
        "main" => t!("admin-credentials-world-main"),
        "isekai" => t!("admin-credentials-world-isekai"),
        "shared" => t!("admin-credentials-world-shared"),
        _ => world.to_owned(),
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
