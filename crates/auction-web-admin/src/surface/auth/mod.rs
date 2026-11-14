//! 管理端认证 surface 的稳定出口。
//! 会话状态编排、站内 return 路径、重试/刷新、登录表单和认证后外壳分别归属子模块；
//! 本文件继续持有认证状态到页面分支的唯一编排入口。

use auction_web_shared::t;
use auction_web_shared::{use_app_boot_ready, use_dialog_focus_trap};
use dioxus::prelude::*;

use super::{AdminReadinessResource, Route, admin_api_error_message};
use crate::api::{
    AdminReadinessState, AdminSession, AdminSessionCheckTrigger, AdminSessionWritePermission,
    fetch_admin_readiness, fetch_admin_session,
};

mod login;
mod navigation;
mod redirect;
mod session;
mod shell;

pub(super) use login::AdminLogin;
pub(super) use redirect::{AdminLoginRedirect, AdminReturnRedirect};
pub(super) use session::AdminSessionRetry;
pub(super) use shell::AdminAuthenticatedShell;

/// 管理端外壳。与公开端外壳完全独立，并沿用旧版的左侧纵向导航布局。
#[component]
pub(super) fn AdminShell() -> Element {
    use_hook(super::router::restore_initial_admin_location);
    // 与 Public 外壳一致：弹窗打开期间锁定背景滚动。
    use_dialog_focus_trap(true);
    // layout 在 `/login` 与认证后页面间不会重挂载；必须订阅 Router 的当前路由，不能直接
    // 读取一次 `window.location`，否则登录成功后的 replace 会把旧重定向分支永久留在 DOM。
    let current_route = use_route::<Route>();
    let is_login_path = matches!(&current_route, Route::Login {});
    let is_dashboard_path = matches!(&current_route, Route::Dashboard {});
    let mut reload_generation = use_signal(|| 0_u64);
    let mut confirmed_session = use_signal(|| None::<AdminSession>);
    let mut identity_epoch = use_signal(|| 0_u64);
    use_context_provider(move || AdminSessionCheckTrigger(reload_generation));
    let readiness = use_resource(fetch_admin_readiness);
    let session = use_resource(move || {
        let generation = reload_generation();
        async move {
            let result = fetch_admin_session().await;
            // 注销或更新检查代数后，迟到结果不能恢复旧身份。
            if generation == *reload_generation.peek()
                && let Ok(current) = &result
            {
                let changed = confirmed_session
                    .peek()
                    .as_ref()
                    .map(|session| (&session.actor, &session.csrf_token))
                    != current.as_ref().map(|session| (&session.actor, &session.csrf_token));
                if changed {
                    let next_epoch = identity_epoch.peek().saturating_add(1);
                    identity_epoch.set(next_epoch);
                }
                confirmed_session.set(current.clone());
            }
            (generation, result)
        }
    });
    let mutation_permission = use_memo(move || {
        session.read().as_ref().is_some_and(|(generation, result)| {
            *generation == reload_generation()
                && matches!(result, Ok(Some(_)))
                && confirmed_session.read().is_some()
        })
    });
    use_context_provider(move || AdminSessionWritePermission(mutation_permission));
    let readiness_context: AdminReadinessResource = readiness;
    use_context_provider(move || readiness_context);
    let (checked_generation, session_snapshot) =
        session.read().clone().map_or((0, None), |(generation, result)| (generation, Some(result)));
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
    if let Some(current) = confirmed_session() {
        // 身份变化必须销毁整棵草稿/命令 Scope；单个静态组件的 key 不会形成 keyed diff。
        // 已认证访问登录地址只重定向，不卸载会话草稿和未决命令宿主。
        return rsx! {
            for (epoch, current) in [(identity_epoch(), current)] {
                AdminAuthenticatedShell {
                    key: "{epoch}",
                    session: current,
                    checked_generation,
                    reload_generation,
                    readiness: readiness_state,
                    session_confirmed: mutation_permission(),
                    redirecting: is_login_path,
                    on_logout: move |_| {
                        confirmed_session.set(None);
                        identity_epoch.set(identity_epoch().saturating_add(1));
                        reload_generation.set(reload_generation().saturating_add(1));
                    },
                }
            }
            if is_login_path {
                AdminReturnRedirect {}
            }
        };
    }

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
            AdminLogin { reload_generation }
        },
        Some(Ok(Some(_))) => rsx! {
            div { role: "status", {t!("admin-session-loading")} }
        },
    }
}
