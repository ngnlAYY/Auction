//! 管理端认证 surface 的稳定出口。
//!
//! 会话状态编排、站内 return 路径、重试/刷新、登录表单和认证后外壳分别归属子模块；
//! 本文件继续持有认证状态到页面分支的唯一编排入口。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;

use auction_web_shared::{
    LanguagePicker, use_app_boot_ready, use_dialog_focus_trap, use_route_focus,
};

use crate::api::{
    AdminReadinessState, AdminSession, fetch_admin_readiness, fetch_admin_session, login_admin,
    logout_admin,
};

use super::{AdminReadinessResource, Route, admin_api_error_message};

mod login;
mod redirect;
mod session;
mod shell;

pub(super) use login::{AdminLogin, AdminReadinessStatus};
pub(super) use redirect::{AdminLoginRedirect, AdminReturnRedirect};
pub(super) use session::{AdminSessionRefresh, AdminSessionRetry};
pub(super) use shell::AdminAuthenticatedShell;

/// 管理端外壳。与公开端外壳完全独立，并沿用旧版的左侧纵向导航布局。
#[component]
pub(super) fn AdminShell() -> Element {
    // 旧 Admin 只限制 dialog 焦点，不锁定 body 滚动；保留滚动条也避免窄屏弹窗宽度跳变。
    use_dialog_focus_trap(false);
    // layout 在 `/login` 与认证后页面间不会重挂载；必须订阅 Router 的当前路由，不能直接
    // 读取一次 `window.location`，否则登录成功后的 replace 会把旧重定向分支永久留在 DOM。
    let current_route = use_route::<Route>();
    let is_login_path = matches!(&current_route, Route::Login {});
    let is_dashboard_path = matches!(&current_route, Route::Dashboard {});
    let mut reload_generation = use_signal(|| 0_u64);
    let readiness = use_resource(fetch_admin_readiness);
    let session = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_session()
    });
    let readiness_context: AdminReadinessResource = readiness;
    use_context_provider(move || readiness_context);
    let session_snapshot = session.read().clone();
    let shell_boot_ready = readiness.read().is_some()
        && match session_snapshot.as_ref() {
            Some(Err(_)) => true,
            Some(Ok(None)) => is_login_path,
            Some(Ok(Some(_))) => !is_login_path && !is_dashboard_path,
            None => false,
        };
    // 已认证 Dashboard 由页面快照解除门禁；重定向路径保持骨架，避免中间页面短暂露出。
    use_app_boot_ready(shell_boot_ready);
    let readiness_state = match &*readiness.read_unchecked() {
        None => AdminReadinessState::Checking,
        Some(Ok(true)) => AdminReadinessState::Ready,
        Some(Ok(false) | Err(_)) => AdminReadinessState::Degraded,
    };

    match session_snapshot.as_ref() {
        None => rsx! {
            div { role: "status", {t!("admin-session-loading")} }
        },
        Some(Err(error)) => rsx! {
            main { class: "admin-login-page",
                section {
                    class: "admin-login-card",
                    role: "alert",
                    aria_labelledby: "admin-session-error-title",
                    h1 { id: "admin-session-error-title", {t!("page-login-title")} }
                    p { class: "admin-panel__note", {admin_api_error_message(error)} }
                    button {
                        r#type: "button",
                        onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                        {t!("admin-retry")}
                    }
                }
            }
            AdminSessionRetry { reload_generation }
        },
        Some(Ok(None)) if !is_login_path => rsx! {
            AdminLoginRedirect {}
        },
        Some(Ok(None)) => rsx! {
            AdminLogin {
                reload_generation,
                readiness: readiness_state,
            }
        },
        Some(Ok(Some(_))) if is_login_path => rsx! {
            AdminReturnRedirect {}
        },
        Some(Ok(Some(current))) => rsx! {
            AdminAuthenticatedShell {
                session: current.clone(),
                reload_generation,
                readiness: readiness_state,
            }
        },
    }
}
