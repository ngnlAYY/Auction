//! 管理端会话、登录和认证后外壳。

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

/// 未认证访问受保护页面时，按旧版保存站内 return 地址并替换到登录页。
#[component]
fn AdminLoginRedirect() -> Element {
    let return_target = current_admin_location();
    let encoded_return =
        url::form_urlencoded::byte_serialize(return_target.as_bytes()).collect::<String>();
    let target = format!("/login?return={encoded_return}");
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

/// 已认证用户访问登录路径时，按旧版 return 参数回到原始站内地址。
#[component]
fn AdminReturnRedirect() -> Element {
    let target = admin_return_path();
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

fn admin_is_login_path() -> bool {
    current_admin_path().as_deref() == Some("/login")
}

#[cfg(target_arch = "wasm32")]
fn current_admin_path() -> Option<String> {
    web_sys::window().and_then(|window| window.location().pathname().ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn current_admin_path() -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
fn current_admin_location() -> String {
    let Some(window) = web_sys::window() else {
        return "/".to_owned();
    };
    let location = window.location();
    let pathname = location.pathname().unwrap_or_else(|_| "/".to_owned());
    let search = location.search().unwrap_or_default();
    let hash = location.hash().unwrap_or_default();
    if pathname.starts_with('/') && pathname != "/login" {
        format!("{pathname}{search}{hash}")
    } else {
        "/".to_owned()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn current_admin_location() -> String {
    "/".to_owned()
}

fn admin_return_path() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return "/".to_owned();
        };
        let search = window.location().search().unwrap_or_default();
        let raw = search.strip_prefix('?').unwrap_or_default();
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            if key == "return"
                && value.starts_with('/')
                && !value.starts_with("//")
                && !value.chars().any(char::is_control)
            {
                return value.into_owned();
            }
        }
    }
    "/".to_owned()
}

/// 会话查询失败时按旧版策略每 30 秒重试；成功后该组件随错误页一起卸载。
#[component]
fn AdminSessionRetry(mut reload_generation: Signal<u64>) -> Element {
    #[cfg(target_arch = "wasm32")]
    use_future(move || async move {
        loop {
            TimeoutFuture::new(30_000).await;
            reload_generation.set(reload_generation().saturating_add(1));
        }
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = reload_generation;
    rsx! {}
}

/// 在 Admin 会话滑动过期前后触发一次 GET，复用服务端的 touch 语义刷新 Cookie 会话。
#[component]
fn AdminSessionRefresh(session: AdminSession, mut reload_generation: Signal<u64>) -> Element {
    #[cfg(target_arch = "wasm32")]
    {
        let delay = session_refresh_delay(&session.expires_at);
        use_future(move || async move {
            TimeoutFuture::new(delay).await;
            reload_generation.set(reload_generation().saturating_add(1));
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (session, reload_generation);
    }
    rsx! {}
}

#[cfg(target_arch = "wasm32")]
fn session_refresh_delay(expires_at: &str) -> u32 {
    let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return 30_000;
    };
    let delay = expiry.timestamp_millis() as f64 - js_sys::Date::now();
    if !delay.is_finite() {
        return 30_000;
    }
    delay.clamp(0.0, 2_147_483_647.0) as u32
}

/// 会话查询尚未完成或暂时失败时的最小安全页面。
#[component]
fn AdminSessionPanel(title: String, detail: String) -> Element {
    rsx! {
        main { class: "admin-login-page", aria_busy: "true",
            section { class: "admin-login-card", aria_labelledby: "admin-session-title",
                h1 { id: "admin-session-title", "{title}" }
                p { class: "admin-panel__note", "{detail}" }
            }
        }
    }
}

/// Admin 登录表单；secret 只在提交时进入一次 Basic Auth 请求。
#[component]
fn AdminLogin(mut reload_generation: Signal<u64>, readiness: AdminReadinessState) -> Element {
    let mut actor = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut submitting = use_signal(|| false);
    let mut failed = use_signal(|| false);

    rsx! {
        main { class: "admin-login-page", aria_labelledby: "admin-login-title",
            section { class: "admin-login-card", aria_labelledby: "admin-login-title",
                h1 { id: "admin-login-title", {t!("page-login-title")} }
                p { class: "admin-panel__note", {t!("page-login-note")} }
                div { class: "admin-visually-hidden",
                    AdminReadinessStatus { state: readiness }
                }
                form {
                    class: "admin-auth-form",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if submitting() || actor().trim().is_empty() || secret().is_empty() {
                            failed.set(true);
                            return;
                        }
                        submitting.set(true);
                        failed.set(false);
                        let submitted_actor = actor();
                        let submitted_secret = secret();
                        secret.set(String::new());
                        spawn(async move {
                            let successful = login_admin(submitted_actor, submitted_secret).await.is_ok();
                            submitting.set(false);
                            if successful {
                                reload_generation.set(reload_generation().saturating_add(1));
                            } else {
                                failed.set(true);
                            }
                        });
                    },
                    label { for: "admin-actor", {t!("admin-login-actor")} }
                    input {
                        id: "admin-actor",
                        r#type: "text",
                        autocomplete: "username",
                        required: true,
                        disabled: submitting(),
                        value: "{actor}",
                        oninput: move |event| actor.set(event.value()),
                    }
                    label { for: "admin-secret", {t!("admin-login-secret")} }
                    input {
                        id: "admin-secret",
                        r#type: "password",
                        autocomplete: "current-password",
                        required: true,
                        disabled: submitting(),
                        value: "{secret}",
                        oninput: move |event| secret.set(event.value()),
                    }
                    button {
                        class: "admin-btn-primary",
                        r#type: "submit",
                        disabled: submitting(),
                        if submitting() { {t!("admin-login-working")} } else { {t!("admin-login-submit")} }
                    }
                }
                if failed() {
                    p { class: "action-feedback action-feedback--error", role: "alert",
                        {t!("admin-login-error")}
                    }
                }
            }
        }
    }
}

/// 已认证的管理端外壳与注销动作。
#[component]
fn AdminAuthenticatedShell(
    session: AdminSession,
    mut reload_generation: Signal<u64>,
    readiness: AdminReadinessState,
) -> Element {
    let mut logging_out = use_signal(|| false);
    let mut logout_failed = use_signal(|| false);
    rsx! {
        div { class: "admin-shell",
            AdminSessionRefresh {
                key: "{session.expires_at}",
                session: session.clone(),
                reload_generation,
            }
            a { class: "admin-shell__skip", href: "#admin-content", {t!("skip")} }
            header { class: "admin-shell__header",
                Link { class: "admin-shell__logo", to: Route::Dashboard {},
                    {t!("admin-brand")}
                }
                div { class: "admin-shell__controls",
                    div { class: "admin-shell__health",
                        AdminReadinessStatus { state: readiness }
                    }
                    LanguagePicker { label: t!("language-label") }
                    button {
                        class: "admin-shell__logout",
                        r#type: "button",
                        disabled: logging_out(),
                        onclick: move |_| {
                            if logging_out() {
                                return;
                            }
                            logging_out.set(true);
                            logout_failed.set(false);
                            let csrf_token = session.csrf_token.clone();
                            spawn(async move {
                                let successful = logout_admin(csrf_token).await.is_ok();
                                logging_out.set(false);
                                if successful {
                                    reload_generation.set(reload_generation().saturating_add(1));
                                } else {
                                    logout_failed.set(true);
                                }
                            });
                        },
                        if logging_out() { {t!("admin-logout-working")} } else { {t!("admin-logout")} }
                    }
                }
            }
            if logout_failed() {
                p { class: "admin-shell__banner", role: "alert", {t!("admin-logout-error")} }
            }
            div { class: "admin-shell__body",
                nav { class: "admin-shell__nav", aria_label: t!("nav-label"),
                    Link { to: Route::Dashboard {}, {t!("nav-dashboard")} }
                    Link { to: Route::Mails {}, {t!("nav-mails")} }
                    Link { to: Route::Auctions {}, {t!("nav-auctions")} }
                    Link { to: Route::Cookies {}, {t!("page-cookies-title")} }
                    Link { to: Route::Users {}, {t!("nav-users")} }
                    Link { to: Route::Renames {}, {t!("page-renames-title")} }
                    Link { to: Route::Deliveries {}, {t!("nav-deliveries")} }
                    Link { to: Route::Outbox {}, {t!("page-outbox-title")} }
                    Link { to: Route::Settlement {}, {t!("nav-settlement")} }
                    Link { to: Route::Config {}, {t!("nav-config")} }
                    Link { to: Route::RulesContent {}, {t!("page-rules-content-title")} }
                    Link { to: Route::Audit {}, {t!("nav-audit")} }
                }
                main { class: "admin-shell__content", id: "admin-content",
                    Outlet::<Route> {}
                }
            }
        }
    }
}

/// 全局 readiness 的无障碍状态提示；视觉布局沿用旧版的隐藏健康提示约定。
#[component]
fn AdminReadinessStatus(state: AdminReadinessState) -> Element {
    let (class, label) = match state {
        AdminReadinessState::Checking => ("checking", t!("admin-readiness-checking")),
        AdminReadinessState::Ready => ("ready", t!("admin-readiness-ready")),
        AdminReadinessState::Degraded => ("degraded", t!("admin-readiness-degraded")),
    };
    rsx! {
        p {
            class: "ui-health-status ui-health-status--{class}",
            "data-state": class,
            aria_live: "polite",
            role: "status",
            span { class: "ui-health-status__indicator", aria_hidden: "true" }
            "{label}"
        }
    }
}
