//! Public 登录和会话页面。
//!
//! 页面不保存或回显服务端 session token；登录成功后只展示安全身份投影，注销仍通过服务端
//! 会话端口完成。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{PublicSession, fetch_public_session, login_public, logout_public};

use super::Placeholder;

#[derive(Debug, Clone, PartialEq)]
enum LoginState {
    Success(PublicSession),
    Failure,
}

pub(super) fn login() -> Element {
    let mut key = use_signal(String::new);
    let mut result = use_signal(|| None::<LoginState>);
    let mut submitting = use_signal(|| false);

    rsx! {
        section { class: "public-auction-empty", aria_labelledby: "login-title",
            h1 { id: "login-title", {t!("page-login-title")} }
            form {
                onsubmit: move |event| {
                    event.prevent_default();
                    if submitting() {
                        return;
                    }
                    let submitted_key = key();
                    if submitted_key.trim().is_empty() {
                        result.set(Some(LoginState::Failure));
                        return;
                    }
                    submitting.set(true);
                    spawn(async move {
                        let login_result = login_public(submitted_key).await;
                        submitting.set(false);
                        result.set(Some(match login_result {
                            Ok(session) => LoginState::Success(session),
                            Err(_) => LoginState::Failure,
                        }));
                    });
                },
                label { for: "public-login-key", {t!("login-key-label")} }
                input {
                    id: "public-login-key",
                    r#type: "password",
                    autocomplete: "current-password",
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
                        p { role: "status", {t!("login-success", username: session.username)} }
                    },
                    LoginState::Failure => rsx! {
                        p { role: "alert", {t!("login-error")} }
                    },
                }
            }
        }
    }
}

pub(super) fn account() -> Element {
    let session = use_resource(fetch_public_session);
    let mut logout_state = use_signal(|| None::<bool>);

    match &*session.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-account-title"), detail: t!("session-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-account-title"), detail: t!("session-error") }
        },
        Some(Ok(None)) => rsx! {
            section { class: "public-auction-empty", aria_labelledby: "account-title",
                h1 { id: "account-title", {t!("page-account-title")} }
                p { {t!("session-anonymous")} }
            }
        },
        Some(Ok(Some(identity))) => {
            let identity = identity.clone();
            rsx! {
                section { class: "public-auction-empty", aria_labelledby: "account-title",
                    h1 { id: "account-title", {t!("page-account-title")} }
                    p { {t!("session-user", username: identity.username.clone())} }
                    p { {t!("session-expires", expires_at: identity.expires_at.clone())} }
                    button {
                        r#type: "button",
                        onclick: move |_| {
                            spawn(async move {
                                logout_state.set(Some(logout_public().await.is_ok()));
                            });
                        },
                        {t!("logout")}
                    }
                    if let Some(successful) = logout_state() {
                        p { role: if successful { "status" } else { "alert" },
                            if successful { {t!("logout-success")} } else { {t!("logout-error")} }
                        }
                    }
                }
            }
        }
    }
}
