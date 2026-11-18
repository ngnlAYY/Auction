//! 管理端认证外壳。会话状态机决定可见宿主与写许可，外壳负责路由和渲染。

use std::rc::Rc;

use auction_web_shared::t;
use auction_web_shared::{use_app_boot_ready, use_dialog_focus_trap};
use dioxus::prelude::*;

use self::effects::use_session;
use self::login::AdminLogin;
use self::redirect::{AdminLoginRedirect, AdminReturnRedirect};
use self::session::Event;
use self::shell::AdminAuthenticatedShell;
use super::router::Route;
use super::{AdminReadinessResource, admin_api_error_message};
use crate::api::{
    AdminReadinessState, AdminSession, AdminSessionCheckTrigger, AdminSessionWritePermission,
    fetch_admin_readiness,
};

mod activity;
mod effects;
mod login;
mod navigation;
mod redirect;
mod session;
mod shell;

/// 管理端外壳。与公开端外壳完全独立，并沿用旧版的左侧纵向导航布局。
#[component]
pub(super) fn AdminShell() -> Element {
    use_hook(super::router::restore_initial_admin_location);
    // 与 Public 外壳一致：弹窗打开期间锁定背景滚动。
    use_dialog_focus_trap(true);
    // layout 在登录与认证路由间不会重挂载，登录成功后的地址变化必须更新当前渲染分支。
    let current_route = use_route::<Route>();
    let is_login_path = matches!(&current_route, Route::Login {});
    let is_dashboard_path = matches!(&current_route, Route::Dashboard {});
    let (view, dispatch) = use_session();
    let recheck = use_callback(move |()| dispatch.call(Event::CheckRequested));
    use_context_provider(move || AdminSessionCheckTrigger(recheck));
    let mutation_permission = use_memo(move || view.read().can_write);
    use_context_provider(move || AdminSessionWritePermission(mutation_permission));

    // 已认证外壳在站内路由切换时递增，顶栏就绪徽标随之重读；重读期间保留旧值，不影响首屏门禁。
    let readiness_refresh = use_signal(|| 0_u64);
    let readiness = use_resource(move || {
        let _ = readiness_refresh();
        fetch_admin_readiness()
    });
    let readiness_context: AdminReadinessResource = readiness;
    use_context_provider(move || readiness_context);
    let snapshot = view();
    let shell_boot_ready = readiness.read().is_some()
        && (snapshot.error.is_some()
            || (snapshot.logged_out && is_login_path)
            || (snapshot.session.is_some() && !is_login_path && !is_dashboard_path));
    // 已认证 Dashboard 由页面快照解除门禁；重定向路径保持骨架，避免中间页面短暂露出。
    use_app_boot_ready(shell_boot_ready);
    let readiness_state = match &*readiness.read_unchecked() {
        None => AdminReadinessState::Checking,
        Some(Ok(true)) => AdminReadinessState::Ready,
        Some(Ok(false) | Err(_)) => AdminReadinessState::Degraded,
    };
    if let Some(current) = snapshot.session {
        // 身份变化必须销毁整棵草稿/命令 Scope；单个静态组件的 key 不会形成 keyed diff。
        // 已认证访问登录地址只重定向，不卸载会话草稿和未决命令宿主。
        return rsx! {
            for (epoch, current) in [(snapshot.epoch, current)] {
                AdminAuthenticatedShell {
                    key: "{epoch}",
                    session: current.as_ref().clone(),
                    readiness: readiness_state,
                    readiness_refresh,
                    session_confirmed: snapshot.can_write,
                    redirecting: is_login_path,
                    logging_out: snapshot.logging_out,
                    logout_error: snapshot.logout_error.clone(),
                    on_logout: move |_| dispatch.call(Event::LogoutClicked),
                    on_recheck: recheck,
                }
            }
            if is_login_path {
                AdminReturnRedirect {}
            }
        };
    }

    if let Some(error) = snapshot.error {
        rsx! {
            main { class: "admin-login-page",
                section {
                    class: "admin-login-card",
                    role: "alert",
                    aria_labelledby: "admin-session-error-title",
                    h1 { id: "admin-session-error-title", {t!("page-login-title")} }
                    p { class: "admin-panel__note", {admin_api_error_message(&error)} }
                    button {
                        r#type: "button",
                        onclick: move |_| recheck.call(()),
                        {t!("admin-retry")}
                    }
                }
            }
        }
    } else if snapshot.logged_out {
        if is_login_path {
            rsx! {
                AdminLogin {
                    on_login: move |session: AdminSession| {
                        dispatch.call(Event::LoginCompleted(Rc::new(session)));
                    },
                }
            }
        } else {
            rsx! { AdminLoginRedirect {} }
        }
    } else {
        rsx! { div { role: "status", {t!("admin-session-loading")} } }
    }
}
