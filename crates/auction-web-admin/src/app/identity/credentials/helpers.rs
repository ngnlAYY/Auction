//! 凭据页面的幂等命令和安全标签映射辅助。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

/// 执行一个管理动作并在成功后触发安全投影刷新。
pub fn run_action<F>(
    mut busy: Signal<bool>,
    mut feedback: Signal<Option<bool>>,
    mut reload_generation: Signal<u64>,
    mut command_state: Signal<Option<(String, Uuid)>>,
    action: F,
) where
    F: std::future::Future<Output = Result<(), String>> + 'static,
{
    busy.set(true);
    feedback.set(None);
    spawn(async move {
        let success = action.await.is_ok();
        feedback.set(Some(success));
        if success {
            command_state.set(None);
            reload_generation.set(reload_generation().saturating_add(1));
        }
        busy.set(false);
    });
}

/// 为同一组表单字段复用 command_id，避免重复提交生成不同命令。
pub fn stable_command_id(
    mut command_state: Signal<Option<(String, Uuid)>>,
    signature: String,
) -> Uuid {
    match command_state() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            command_state.set(Some((signature, command_id)));
            command_id
        }
    }
}

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

pub fn credential_status_action_label(status: &str) -> String {
    match status {
        "active" => t!("admin-credentials-action-disable"),
        "disabled" => t!("admin-credentials-action-enable"),
        _ => t!("admin-credentials-action-status"),
    }
}
