//! Public 登录和会话页面。
//!
//! 页面不保存或回显服务端 session token；登录成功后只展示安全身份投影，注销仍通过服务端
//! 会话端口完成。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use url::form_urlencoded;

use crate::api::{PublicSession, PublicSessionApiError, login_public, logout_public};

use super::{PublicSessionResource, Route};

#[derive(Debug, Clone, PartialEq)]
enum LoginState {
    Success(PublicSession),
    Failure(PublicSessionApiError),
}

#[derive(Clone)]
enum SessionMenuState {
    Anonymous,
    Authenticated(String),
}

#[derive(Clone)]
pub(crate) struct InitialLoginReturnLocation(String);

/// 在 Router 规范化地址前保存首次同源 pathname + search。
pub(crate) fn initial_login_return_location() -> InitialLoginReturnLocation {
    let value = browser_path_and_search().unwrap_or_else(|| "/".to_owned());
    InitialLoginReturnLocation(safe_return_path(Some(&value)))
}

pub(super) fn login(return_to: Option<String>) -> Element {
    let mut key = use_signal(String::new);
    let mut result = use_signal(|| None::<LoginState>);
    let mut submitting = use_signal(|| false);
    let mut session_resource = use_context::<PublicSessionResource>();
    let navigator = use_navigator();
    let return_path = safe_return_path(return_to.as_deref());
    let failed = matches!(result(), Some(LoginState::Failure(_)));

    rsx! {
        section {
            class: "public-account-login-page",
            aria_labelledby: "login-title",
            aria_busy: submitting().to_string(),
            h1 { id: "login-title", {t!("page-login-title")} }
            form {
                class: "public-account-login-form",
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
                    let return_path = return_path.clone();
                    spawn(async move {
                        let login_result = login_public(submitted_key).await;
                        submitting.set(false);
                        match login_result {
                            Ok(session) => {
                                result.set(Some(LoginState::Success(session.clone())));
                                session_resource.set(Some(Ok(Some(session))));
                                let _ = navigator.replace(return_path);
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
                    placeholder: if submitting() || failed {
                        "••••••••••••••••••••••••••••••••".to_owned()
                    } else {
                        t!("login-key-placeholder")
                    },
                    "data-masked": (submitting() || failed).to_string(),
                    disabled: submitting(),
                    value: "{key}",
                    oninput: move |event| key.set(event.value()),
                }
                if let Some(LoginState::Failure(error)) = result() {
                    p { class: "public-account-login-error", role: "alert",
                        span { {login_error_message(&error)} }
                        if let Some(reason) = login_blacklisted_reason(&error) {
                            span { class: "public-account-login-ban-reason",
                                {t!("login-blacklisted-reason", reason: reason)}
                            }
                        }
                    }
                }
                button {
                    r#type: "submit",
                    disabled: submitting() || key().trim().is_empty(),
                    "data-error": failed.to_string(),
                    if submitting() { {t!("login-working")} } else { {t!("login-submit")} }
                }
            }
            p { class: "public-account-login-hint", {t!("login-hint")} }
            if let Some(LoginState::Success(session)) = result() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {t!("login-success", username: session.username)}
                }
            }
        }
    }
}

/// 为需要认证的 CTA 保存当前 pathname 与 search；直接登录入口不携带返回地址。
pub(crate) fn login_href_for_current_location() -> String {
    let Some(return_to) = current_path_and_search() else {
        return "/login".to_owned();
    };
    // 旧版用 Router state 保存来源；根路径本就是登录页默认返回值，地址栏保持裸 `/login`。
    if return_to == "/" {
        return "/login".to_owned();
    }
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("return_to", &return_to)
        .finish();
    format!("/login?{query}")
}

fn safe_return_path(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "/".to_owned();
    };
    if !value.starts_with('/')
        || value.starts_with("//")
        || value.starts_with("/login?")
        || value == "/login"
        || value.contains(['\\', '#'])
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return "/".to_owned();
    }
    value.to_owned()
}

fn current_path_and_search() -> Option<String> {
    let current = browser_path_and_search()?;
    if current.contains('?') {
        return (safe_return_path(Some(&current)) == current).then_some(current);
    }
    if let Some(initial) = try_consume_context::<InitialLoginReturnLocation>()
        && initial.0.split('?').next() == Some(current.as_str())
        && safe_return_path(Some(&initial.0)) == initial.0
    {
        return Some(initial.0);
    }
    (safe_return_path(Some(&current)) == current).then_some(current)
}

fn browser_path_and_search() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let location = web_sys::window()?.location();
        let pathname = location.pathname().ok()?;
        let search = location.search().unwrap_or_default();
        Some(format!("{pathname}{search}"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    None
}

/// 页头会话菜单；匿名状态保留旧版的“我的竞价 + 登录”入口，认证状态显示用户名和退出。
#[component]
pub(super) fn SessionMenu() -> Element {
    let mut session = use_context::<PublicSessionResource>();
    let mut logout_error = use_signal(|| false);
    let mut logout_busy = use_signal(|| false);

    let session_state = match &*session.read() {
        Some(Ok(Some(identity))) if identity.authenticated => {
            SessionMenuState::Authenticated(identity.username.clone())
        }
        None | Some(Err(_)) | Some(Ok(None)) | Some(Ok(Some(_))) => SessionMenuState::Anonymous,
    };

    rsx! {
        div { class: "public-account-session-menu", aria_label: t!("session-menu-label"),
            {match session_state {
                SessionMenuState::Anonymous => rsx! {
                    Link {
                        to: Route::Me {},
                        {t!("nav-my-bids")}
                    }
                    Link {
                        class: "public-account-session-login",
                        // typed query route 会把空参数序列化成 `/login?`；直接入口保持旧版精确 URL。
                        to: "/login",
                        {t!("session-login")}
                    }
                },
                SessionMenuState::Authenticated(username) => rsx! {
                    Link {
                        class: "public-account-session-name",
                        to: Route::Me {},
                        {username}
                    }
                    button {
                        class: "public-account-session-logout",
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
                                    session.set(Some(Ok(None)));
                                } else {
                                    logout_error.set(true);
                                }
                                logout_busy.set(false);
                            });
                        },
                        {t!("logout")}
                    }
                },
            }}
            if logout_error() {
                span { class: "public-visually-hidden", role: "alert", {t!("logout-error")} }
            }
        }
    }
}

fn login_error_message(error: &PublicSessionApiError) -> String {
    match error.code.as_deref() {
        Some("empty_key" | "invalid_key") => t!("common-error"),
        Some("invalid_credentials") => t!("login-invalid-credentials"),
        Some("account_blacklisted") if error.status == Some(403) => t!("login-blacklisted"),
        Some("session_limit") => t!("login-session-limit"),
        Some("invalid_response") => t!("login-invalid-response"),
        Some("network_error") => t!("login-network-error"),
        Some("request_timeout") => t!("login-request-timeout"),
        _ => t!("common-error"),
    }
}

fn login_blacklisted_reason(error: &PublicSessionApiError) -> Option<&str> {
    (error.status == Some(403) && error.code.as_deref() == Some("account_blacklisted"))
        .then_some(error.public_reason.as_deref())
        .flatten()
}
