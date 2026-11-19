//! Admin 登录表单，以及外壳顶栏使用的就绪状态徽标。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::AdminReadinessState;
use crate::surface::admin_api_error_message;

/// 只交给外壳使用一次的登录凭据。
pub(super) struct LoginSubmission {
    pub(super) actor: String,
    pub(super) secret: String,
}

/// Admin 登录表单；secret 只在提交时进入一次 Basic Auth 请求。
#[component]
pub(super) fn AdminLogin(
    submitting: bool,
    login_error: Option<String>,
    on_edit: EventHandler<()>,
    on_submit: EventHandler<LoginSubmission>,
) -> Element {
    let mut actor = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut validation_error = use_signal(|| None::<String>);
    let mut actor_input = use_signal(|| None::<MountedEvent>);
    let mut secret_input = use_signal(|| None::<MountedEvent>);
    use_effect(use_reactive((&submitting, &login_error), move |(submitting, login_error)| {
        if !submitting && (validation_error.read().is_some() || login_error.is_some()) {
            let input = if actor.peek().trim().is_empty() { actor_input() } else { secret_input() };
            if let Some(input) = input {
                spawn(async move {
                    let _ = input.set_focus(true).await;
                });
            }
        }
    }));
    let error_code = validation_error().or(login_error);
    let invalid = error_code.is_some();

    rsx! {
        main { class: "admin-login-page", aria_labelledby: "admin-login-title",
            section { class: "admin-login-card", aria_labelledby: "admin-login-title",
                div { class: "admin-login-brand",
                    span { class: "admin-login-mark", aria_hidden: "true" }
                    div { class: "admin-login-brand__text",
                        p { class: "admin-login-brand__eyebrow", "HV-Auction" }
                        h1 { id: "admin-login-title", {t!("page-login-title")} }
                    }
                }
                form {
                    class: "admin-auth-form",
                    aria_busy: submitting.to_string(),
                    onsubmit: move |event| {
                        event.prevent_default();
                        if submitting {
                            return;
                        }
                        if actor().trim().is_empty() || secret().is_empty() {
                            validation_error.set(Some("invalid_request".to_owned()));
                            return;
                        }
                        validation_error.set(None);
                        let submitted_actor = actor();
                        let submitted_secret = secret();
                        secret.set(String::new());
                        on_submit.call(LoginSubmission { actor: submitted_actor, secret: submitted_secret });
                    },
                    label { for: "admin-actor", {t!("admin-login-actor")} }
                    input {
                        id: "admin-actor",
                        name: "username",
                        r#type: "text",
                        autocomplete: "username",
                        autocapitalize: "none",
                        autocorrect: "off",
                        spellcheck: "false",
                        enterkeyhint: "next",
                        required: true,
                        disabled: submitting,
                        aria_invalid: invalid.to_string(),
                        aria_describedby: invalid.then_some("admin-login-error"),
                        onmounted: move |event| actor_input.set(Some(event)),
                        value: "{actor}",
                        oninput: move |event| {
                            actor.set(event.value());
                            validation_error.set(None);
                            on_edit.call(());
                        },
                    }
                    label { for: "admin-secret", {t!("admin-login-secret")} }
                    input {
                        id: "admin-secret",
                        name: "password",
                        r#type: "password",
                        autocomplete: "current-password",
                        enterkeyhint: "go",
                        required: true,
                        disabled: submitting,
                        aria_invalid: invalid.to_string(),
                        aria_describedby: invalid.then_some("admin-login-error"),
                        onmounted: move |event| secret_input.set(Some(event)),
                        value: "{secret}",
                        oninput: move |event| {
                            secret.set(event.value());
                            validation_error.set(None);
                            on_edit.call(());
                        },
                    }
                    button {
                        class: "admin-btn-primary",
                        r#type: "submit",
                        disabled: submitting,
                        aria_busy: submitting.to_string(),
                        if submitting { {t!("admin-login-working")} } else { {t!("admin-login-submit")} }
                    }
                }
                if let Some(code) = error_code {
                    p { id: "admin-login-error", class: "action-feedback action-feedback--error", role: "alert",
                        {login_error_message(&code)}
                    }
                }
            }
        }
    }
}

fn login_error_message(code: &str) -> String {
    // 登录 POST 没有业务命令 ID，超时反馈不能承诺保留密码或幂等重试。
    if code == "request_timeout" {
        return t!("admin-error-reconciliation-required");
    }
    admin_api_error_message(code)
}

/// 全局 readiness 状态徽标，显示在已登录外壳的顶栏；圆点与降级文字按状态着色。
#[component]
pub(crate) fn AdminReadinessStatus(state: AdminReadinessState) -> Element {
    let (class, label) = match state {
        AdminReadinessState::Checking => ("checking", t!("admin-readiness-checking")),
        AdminReadinessState::Ready => ("ready", t!("admin-readiness-ready")),
        AdminReadinessState::Degraded => ("degraded", t!("admin-readiness-degraded")),
    };
    rsx! {
        p {
            class: "admin-readiness-status admin-readiness-status--{class}",
            "data-state": class,
            aria_live: "polite",
            role: "status",
            span { class: "admin-readiness-status__indicator", aria_hidden: "true" }
            "{label}"
        }
    }
}
