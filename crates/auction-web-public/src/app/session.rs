//! Public 登录和会话页面。
//!
//! 页面不保存或回显服务端 session token；登录成功后只展示安全身份投影，注销仍通过服务端
//! 会话端口完成。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    PublicSession, PublicSessionApiError, fetch_public_session, login_public, logout_public,
};

use super::Route;

#[derive(Debug, Clone, PartialEq)]
enum LoginState {
    Success(PublicSession),
    Failure(PublicSessionApiError),
}

pub(super) fn login() -> Element {
    let mut key = use_signal(String::new);
    let mut result = use_signal(|| None::<LoginState>);
    let mut submitting = use_signal(|| false);
    let mut refresh_generation = use_context::<Signal<u64>>();

    rsx! {
        section {
            class: "public-auction-empty",
            aria_labelledby: "login-title",
            aria_busy: submitting().to_string(),
            h1 { id: "login-title", {t!("page-login-title")} }
            form {
                onsubmit: move |event| {
                    event.prevent_default();
                    if submitting() {
                        return;
                    }
                    let submitted_key = key().trim().to_owned();
                    if submitted_key.is_empty() {
                        result.set(Some(LoginState::Failure(PublicSessionApiError::local(
                            "empty_key",
                        ))));
                        return;
                    }
                    // 原始 key 只用于这一次请求；成功或失败都不继续留在表单状态中。
                    key.set(String::new());
                    submitting.set(true);
                    spawn(async move {
                        let login_result = login_public(submitted_key).await;
                        submitting.set(false);
                        match login_result {
                            Ok(session) => {
                                result.set(Some(LoginState::Success(session)));
                                refresh_generation.set(refresh_generation().saturating_add(1));
                                return_to_previous_page();
                            }
                            Err(error) => result.set(Some(LoginState::Failure(error))),
                        }
                    });
                },
                label { for: "public-login-key", {t!("login-key-label")} }
                input {
                    id: "public-login-key",
                    r#type: "password",
                    autocomplete: "off",
                    maxlength: "1024",
                    disabled: submitting(),
                    value: "{key}",
                    oninput: move |event| key.set(event.value()),
                }
                button { r#type: "submit", disabled: submitting(),
                    if submitting() { {t!("login-working")} } else { {t!("login-submit")} }
                }
            }
            if let Some(state) = result() {
                match state {
                    LoginState::Success(session) => rsx! {
                        p { class: "action-feedback action-feedback--success", role: "status", {t!("login-success", username: session.username)} }
                    },
                    LoginState::Failure(error) => rsx! {
                        p { class: "action-feedback action-feedback--error", role: "alert",
                            {login_error_message(&error)}
                        }
                    },
                }
            }
        }
    }
}

/// 登录成功后回到用户发起登录的位置；直接打开登录页时回到公开账户页。
fn return_to_previous_page() {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(history) = window.history() else {
            return;
        };
        if history.length().unwrap_or_default() > 1 {
            let _ = history.back();
        } else {
            let _ = window.location().set_href("/me");
        }
    }
}

/// 页头会话菜单；匿名状态保留旧版的“我的竞价 + 登录”入口，认证状态显示用户名和退出。
#[component]
pub(super) fn SessionMenu() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let session = use_resource(move || {
        let _ = refresh_generation();
        fetch_public_session()
    });
    let mut logout_error = use_signal(|| false);
    let mut logout_busy = use_signal(|| false);

    let authenticated = matches!(
        &*session.read_unchecked(),
        Some(Ok(Some(identity))) if identity.authenticated
    );
    let username = match &*session.read_unchecked() {
        Some(Ok(Some(identity))) if identity.authenticated => Some(identity.username.clone()),
        _ => None,
    };

    rsx! {
        div { class: "public-session-menu", aria_label: t!("session-menu-label"),
            Link {
                class: "public-shell__utility-link public-shell__utility-link--account",
                to: Route::Me {},
                if authenticated {
                    {username.unwrap_or_else(|| t!("nav-my-bids"))}
                } else {
                    {t!("nav-my-bids")}
                }
            }
            if authenticated {
                button {
                    class: "public-shell__utility-link public-session-menu__logout",
                    r#type: "button",
                    disabled: logout_busy(),
                    onclick: move |_| {
                        if logout_busy() {
                            return;
                        }
                        logout_error.set(false);
                        logout_busy.set(true);
                        spawn(async move {
                            if logout_public().await.is_ok() {
                                refresh_generation.set(refresh_generation().saturating_add(1));
                            } else {
                                logout_error.set(true);
                            }
                            logout_busy.set(false);
                        });
                    },
                    if logout_busy() { {t!("logout-working")} } else { {t!("logout")} }
                }
            } else {
                Link {
                    class: "public-shell__utility-link",
                    to: Route::Login {},
                    {t!("session-login")}
                }
            }
            if logout_error() {
                span { class: "public-session-menu__error", role: "alert", {t!("logout-error")} }
            }
        }
    }
}

fn login_error_message(error: &PublicSessionApiError) -> String {
    if error.status == Some(403) && error.code.as_deref() == Some("account_blacklisted") {
        if let Some(reason) = error.public_reason.as_deref() {
            return t!("login-blacklisted-reason", reason: reason);
        }
        return t!("login-blacklisted");
    }
    match error.code.as_deref() {
        Some("empty_key") => t!("login-error"),
        Some("invalid_key") => t!("login-invalid-key"),
        Some("invalid_credentials") => t!("login-invalid-credentials"),
        Some("session_limit") => t!("login-session-limit"),
        _ => t!("login-unavailable"),
    }
}
