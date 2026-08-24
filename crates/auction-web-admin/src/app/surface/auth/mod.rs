//! 管理端认证 surface 的稳定出口。
//!
//! 会话状态编排、站内 return 路径、重试/刷新、登录表单和认证后外壳分别归属子模块；
//! 本文件继续持有认证状态到页面分支的唯一编排入口。

use dioxus::prelude::*;
use dioxus_i18n::t;

#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;

use auction_web_shared::{LanguagePicker, use_dialog_focus_trap};

use crate::api::{
    AdminReadinessState, AdminSession, fetch_admin_readiness, fetch_admin_session, login_admin,
    logout_admin,
};

use super::Route;

mod login;
mod redirect;
mod session;
mod shell;

pub(super) use login::{AdminLogin, AdminReadinessStatus, AdminSessionPanel};
pub(super) use redirect::{AdminLoginRedirect, AdminReturnRedirect, admin_is_login_path};
pub(super) use session::{AdminSessionRefresh, AdminSessionRetry};
pub(super) use shell::AdminAuthenticatedShell;

/// 管理端外壳。与公开端外壳完全独立，并沿用旧版的左侧纵向导航布局。
#[component]
pub(super) fn AdminShell() -> Element {
    use_dialog_focus_trap();
    let mut reload_generation = use_signal(|| 0_u64);
    let readiness = use_resource(fetch_admin_readiness);
    let session = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_session()
    });
    let readiness_state = match &*readiness.read_unchecked() {
        None => AdminReadinessState::Checking,
        Some(Ok(true)) => AdminReadinessState::Ready,
        Some(Ok(false) | Err(_)) => AdminReadinessState::Degraded,
    };

    match &*session.read_unchecked() {
        None => rsx! {
            AdminSessionPanel { title: t!("page-login-title"), detail: t!("admin-session-loading") }
        },
        Some(Err(_)) => rsx! {
            section {
                class: "admin-login-page",
                role: "alert",
                aria_labelledby: "admin-session-error-title",
                h1 { id: "admin-session-error-title", {t!("page-login-title")} }
                p { class: "admin-panel__note", {t!("admin-session-error")} }
                button {
                    r#type: "button",
                    onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                    {t!("admin-retry")}
                }
            }
            AdminSessionRetry { reload_generation }
        },
        Some(Ok(None)) if !admin_is_login_path() => rsx! {
            AdminLoginRedirect {}
        },
        Some(Ok(None)) => rsx! {
            AdminLogin {
                reload_generation,
                readiness: readiness_state,
            }
        },
        Some(Ok(Some(_))) if admin_is_login_path() => rsx! {
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
