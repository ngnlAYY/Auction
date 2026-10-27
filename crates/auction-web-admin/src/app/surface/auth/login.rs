use super::*;

/// Admin 登录表单；secret 只在提交时进入一次 Basic Auth 请求。
#[component]
pub(crate) fn AdminLogin(
    mut reload_generation: Signal<u64>,
    readiness: AdminReadinessState,
) -> Element {
    let mut actor = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut submitting = use_signal(|| false);
    let mut error_code = use_signal(|| None::<String>);
    let mut actor_input = use_signal(|| None::<MountedEvent>);
    let mut secret_input = use_signal(|| None::<MountedEvent>);
    use_effect(move || {
        if error_code.read().is_some() {
            let input = if actor.peek().trim().is_empty() { actor_input() } else { secret_input() };
            if let Some(input) = input {
                spawn(async move {
                    let _ = input.set_focus(true).await;
                });
            }
        }
    });

    rsx! {
        main { class: "admin-login-page", aria_labelledby: "admin-login-title",
            section { class: "admin-login-card", aria_labelledby: "admin-login-title",
                h1 { id: "admin-login-title", {t!("page-login-title")} }
                div { class: "admin-visually-hidden",
                    AdminReadinessStatus { state: readiness }
                }
                form {
                    class: "admin-auth-form",
                    aria_busy: submitting().to_string(),
                    onsubmit: move |event| {
                        event.prevent_default();
                        if submitting() {
                            return;
                        }
                        if actor().trim().is_empty() || secret().is_empty() {
                            error_code.set(Some("invalid_request".to_owned()));
                            return;
                        }
                        submitting.set(true);
                        error_code.set(None);
                        let submitted_actor = actor();
                        let submitted_secret = secret();
                        secret.set(String::new());
                        spawn(async move {
                            let result = login_admin(submitted_actor, submitted_secret).await;
                            submitting.set(false);
                            match result {
                                Ok(_) => {
                                    reload_generation.set(reload_generation().saturating_add(1));
                                }
                                Err(code) => error_code.set(Some(code)),
                            }
                        });
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
                        disabled: submitting(),
                        aria_invalid: error_code.read().is_some().to_string(),
                        aria_describedby: error_code.read().is_some().then_some("admin-login-error"),
                        onmounted: move |event| actor_input.set(Some(event)),
                        value: "{actor}",
                        oninput: move |event| {
                            actor.set(event.value());
                            error_code.set(None);
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
                        disabled: submitting(),
                        aria_invalid: error_code.read().is_some().to_string(),
                        aria_describedby: error_code.read().is_some().then_some("admin-login-error"),
                        onmounted: move |event| secret_input.set(Some(event)),
                        value: "{secret}",
                        oninput: move |event| {
                            secret.set(event.value());
                            error_code.set(None);
                        },
                    }
                    button {
                        class: "admin-btn-primary",
                        r#type: "submit",
                        disabled: submitting(),
                        aria_busy: submitting().to_string(),
                        if submitting() { {t!("admin-login-working")} } else { {t!("admin-login-submit")} }
                    }
                }
                if let Some(code) = error_code() {
                    p { id: "admin-login-error", class: "action-feedback action-feedback--error", role: "alert",
                        {admin_api_error_message(&code)}
                    }
                }
            }
        }
    }
}

/// 全局 readiness 的无障碍状态提示；视觉布局沿用旧版的隐藏健康提示约定。
#[component]
pub(crate) fn AdminReadinessStatus(state: AdminReadinessState) -> Element {
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
