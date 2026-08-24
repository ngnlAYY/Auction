use super::*;

/// 会话查询尚未完成或暂时失败时的最小安全页面。
#[component]
pub(crate) fn AdminSessionPanel(title: String, detail: String) -> Element {
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
pub(crate) fn AdminLogin(
    mut reload_generation: Signal<u64>,
    readiness: AdminReadinessState,
) -> Element {
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
