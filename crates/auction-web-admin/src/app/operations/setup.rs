//! 首次部署页面编排：部署码验证通过后才挂载配置表单，不进入普通 Admin 会话外壳。

#[path = "setup/draft.rs"]
mod draft;
#[path = "setup/fields.rs"]
mod fields;
#[path = "setup/form.rs"]
mod form;
#[path = "setup/validation.rs"]
mod validation;

use auction_web_shared::{LanguagePicker, legacy_t as t, use_app_boot_ready};
use dioxus::prelude::*;

use self::{draft::SetupDraft, form::ConfigurationForm};
use crate::api::verify_first_run;

/// 部署码和草稿只属于当前页面；刷新需重新验证，后续请求仍由服务端独立鉴权。
pub(super) fn page() -> Element {
    use_app_boot_ready(true);
    let mut token = use_signal(String::new);
    let mut verified = use_signal(|| false);
    let mut draft = use_signal(SetupDraft::default);
    let mut busy = use_signal(|| false);
    let mut notice = use_signal(|| None::<(bool, String)>);
    let mut completed = use_signal(|| false);
    let mut initialize_and_start = use_signal(|| false);
    // 页面内不可逆的停靠状态；输入变化或组件重渲染不能解除结果不确定的门禁。
    let mut outcome_unknown = use_signal(|| false);

    rsx! {
        main { class: "admin-setup", aria_labelledby: "admin-setup-title",
            div { class: if verified() { "admin-setup__card" } else { "admin-setup__card admin-setup__card--access" },
                LanguagePicker { label: t!("language-label"), default_locale: "zh-CN" }
                h1 { id: "admin-setup-title", {t!("setup-title")} }
                if !verified() {
                    p { class: "admin-panel__note", {t!("setup-intro")} }
                    form {
                        class: "admin-setup__form",
                        onsubmit: move |event| {
                            event.prevent_default();
                            if busy() || verified() || completed() || outcome_unknown() || token().trim().is_empty() {
                                return;
                            }
                            busy.set(true);
                            notice.set(None);
                            let submitted_token = token();
                            spawn(async move {
                                match verify_first_run(&submitted_token).await {
                                    Ok(initialize) => {
                                        initialize_and_start.set(initialize);
                                        verified.set(true);
                                    },
                                    Err(code) => {
                                        if code == "setup_outcome_unknown" {
                                            outcome_unknown.set(true);
                                            token.set(String::new());
                                            draft.write().clear_passwords();
                                        }
                                        notice.set(Some((false, error_key(&code).to_owned())));
                                    }
                                }
                                busy.set(false);
                            });
                        },
                        fieldset { disabled: busy() || outcome_unknown(),
                            legend { {t!("setup-access")} }
                            label { r#for: "setup-token", {t!("setup-token")} }
                            input {
                                id: "setup-token",
                                r#type: "password",
                                autocomplete: "off",
                                required: true,
                                value: "{token}",
                                oninput: move |event| {
                                    if busy() || outcome_unknown() { return; }
                                    token.set(event.value());
                                    notice.set(None);
                                },
                            }
                        }
                        div { class: "admin-setup__actions",
                            button {
                                class: "admin-btn-primary",
                                r#type: "submit",
                                disabled: busy() || outcome_unknown() || token().trim().is_empty(),
                                if busy() { {t!("setup-verifying")} } else { {t!("setup-verify")} }
                            }
                        }
                    }
                } else if !completed() {
                    if !outcome_unknown() {
                        p { class: "admin-panel__note", {t!("setup-verified")} }
                    }
                    ConfigurationForm {
                        token,
                        draft,
                        notice,
                        outcome_unknown,
                        initialize_and_start: initialize_and_start(),
                        on_auth_failure: move |_| {
                            draft.write().clear_passwords();
                            verified.set(false);
                            token.set(String::new());
                        },
                        on_completed: move |_| {
                            token.set(String::new());
                            completed.set(true);
                        },
                    }
                }
                if completed() && initialize_and_start() {
                    a { href: draft.read().value(draft::SetupField::AdminOrigin).to_owned(), {t!("setup-open-admin")} }
                }
                if let Some((success, key)) = notice() {
                    p {
                        class: if success { "action-feedback" } else { "action-feedback action-feedback--error" },
                        role: if success { "status" } else { "alert" },
                        {t!(&key)}
                    }
                }
            }
        }
    }
}

fn error_key(code: &str) -> &'static str {
    match code {
        "invalid_input" => "setup-error-input",
        "setup_origin" => "setup-error-origin",
        "setup_confirmation_required" => "setup-confirm-initialize",
        "database_unavailable" => "setup-error-database",
        "configuration_invalid" => "setup-error-config",
        "setup_locked" | "setup_completed" => "setup-error-locked",
        "setup_busy" => "setup-error-busy",
        "setup_auth" => "setup-error-token",
        "setup_io" => "setup-error-io",
        "setup_outcome_unknown" => "setup-error-outcome-unknown",
        "network" | "network_error" | "request_timeout" => "setup-error-network",
        _ => "setup-error-unknown",
    }
}
