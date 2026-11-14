//! 首次部署配置步骤：分组输入、统一提交校验和短生命周期异步状态。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{
    draft::{SetupDraft, SetupField},
    error_key,
    fields::{SetupErrors, SetupInput},
    validation::{self, SetupIssue},
};
use crate::{api::submit_first_run, surface::validation::focus_field};

#[component]
pub(super) fn ConfigurationForm(
    mut token: Signal<String>,
    mut draft: Signal<SetupDraft>,
    mut notice: Signal<Option<(bool, String)>>,
    mut outcome_unknown: Signal<bool>,
    initialize_and_start: bool,
    on_auth_failure: EventHandler<()>,
    on_completed: EventHandler<()>,
) -> Element {
    let mut pending = use_signal(|| None::<bool>);
    let mut issues = use_signal(Vec::<SetupIssue>::new);
    let mut advanced_open = use_signal(|| false);
    let mut confirm_initialize = use_signal(|| false);
    let submit = use_callback(move |complete: bool| {
        // 回调内也检查门禁，防止键盘提交或尚未重渲染的按钮绕过 disabled。
        if pending.read().is_some() || outcome_unknown() {
            return;
        }
        if complete && initialize_and_start && !confirm_initialize() {
            notice.set(Some((false, "setup-confirm-initialize".to_owned())));
            return;
        }
        notice.set(None);
        let mut request = match validation::validate(&draft.read()) {
            Ok(request) => request,
            Err(errors) => {
                if errors.iter().any(|issue| SetupField::ADVANCED.contains(&issue.field)) {
                    advanced_open.set(true);
                }
                if let Some(first) = errors.first() {
                    focus_field(first.field.id());
                }
                issues.set(errors);
                return;
            }
        };
        request.confirm_initialize = initialize_and_start && confirm_initialize();
        issues.write().clear();
        pending.set(Some(complete));
        let submitted_token = token();
        spawn(async move {
            let result = submit_first_run(&submitted_token, &request, complete).await;
            pending.set(None);
            match result {
                Ok(()) if complete => {
                    draft.write().clear_passwords();
                    notice.set(Some((
                        true,
                        if initialize_and_start { "setup-starting" } else { "setup-complete" }
                            .to_owned(),
                    )));
                    on_completed.call(());
                }
                Ok(()) => notice.set(Some((true, "setup-valid".to_owned()))),
                Err(code) => {
                    notice.set(Some((false, error_key(&code).to_owned())));
                    if code == "setup_outcome_unknown" {
                        outcome_unknown.set(true);
                        token.set(String::new());
                        draft.write().clear_passwords();
                    } else if code == "setup_auth" {
                        on_auth_failure.call(());
                    }
                }
            }
        });
    });
    let waiting = pending.read().is_some();
    // 终态禁用字段和动作，但不能向辅助技术声称仍有请求正在等待。
    let busy = waiting || outcome_unknown();

    rsx! {
        form {
            class: "admin-setup__form",
            // 两个动作统一走 Rust 校验，避免原生校验跳过按钮或阻止隐藏高级字段的错误定位。
            novalidate: true,
            aria_busy: waiting.to_string(),
            oninput: move |_| { if !busy && !outcome_unknown() { notice.set(None); } },
            onsubmit: move |event| {
                event.prevent_default();
                submit.call(true);
            },
            p { class: "admin-setup__hint", {t!("setup-form-help")} }
            SetupErrors { issues, advanced_open }
            for (legend, fields) in [
                ("setup-database", SetupField::DATABASE.as_slice()),
                ("setup-admin", SetupField::ADMIN.as_slice()),
                ("setup-basic", SetupField::BASIC.as_slice()),
                ("setup-listener", SetupField::LISTENER.as_slice()),
            ] {
                fieldset { key: "{legend}", disabled: busy,
                    legend { {t!(legend)} }
                    if legend == "setup-database" {
                        p { class: "admin-setup__hint", {t!("setup-database-help")} }
                    } else if legend == "setup-listener" {
                        p { class: "admin-setup__hint", {t!("setup-listener-help")} }
                    }
                    div { class: "admin-setup__grid",
                        for field in fields.iter().copied() {
                            SetupInput { key: "{field.id()}", field, draft, issues, busy }
                        }
                    }
                    if legend == "setup-basic" {
                        div { class: "admin-setup__options",
                            label { class: "admin-setup__check",
                                input {
                                    r#type: "checkbox",
                                    checked: draft.read().metrics_enabled,
                                    aria_describedby: "setup-metrics-help",
                                    onchange: move |event| {
                                        if busy { return; }
                                        draft.write().metrics_enabled = event.checked();
                                        notice.set(None);
                                    },
                                }
                                {t!("setup-metrics")}
                            }
                            p { id: "setup-metrics-help", class: "admin-setup__hint", {t!("setup-metrics-help")} }
                        }
                    }
                }
            }
            fieldset { disabled: busy,
                legend { {t!("setup-schedule-settings")} }
                label { class: "admin-setup__check",
                    input {
                        r#type: "checkbox",
                        checked: draft.read().schedule_enabled,
                        aria_describedby: "setup-schedule-help",
                        onchange: move |event| {
                            if pending.read().is_some() || outcome_unknown() { return; }
                            draft.write().schedule_enabled = event.checked();
                            issues.write().retain(|issue| !SetupField::SCHEDULE.contains(&issue.field));
                            notice.set(None);
                        },
                    }
                    {t!("setup-schedule")}
                }
                p { id: "setup-schedule-help", class: "admin-setup__hint", {t!("setup-schedule-help")} }
                SetupInput { field: SetupField::ScheduleTimezone, draft, issues, busy }
                div { class: "admin-setup__grid",
                    for field in SetupField::SCHEDULE.into_iter().skip(1) {
                        SetupInput { key: "{field.id()}", field, draft, issues, busy }
                    }
                }
            }
            div { class: "admin-setup__advanced",
                button {
                    class: "admin-setup__toggle",
                    r#type: "button",
                    disabled: busy,
                    aria_expanded: advanced_open().to_string(),
                    aria_controls: "setup-advanced-fields",
                    onclick: move |_| advanced_open.toggle(),
                    if advanced_open() { {t!("setup-advanced-hide")} } else { {t!("setup-advanced-show")} }
                }
                p { class: "admin-setup__hint", {t!("setup-advanced-help")} }
                div { id: "setup-advanced-fields", hidden: !advanced_open(),
                    fieldset { disabled: busy,
                        legend { {t!("setup-advanced")} }
                        button {
                            class: "admin-setup__reset",
                            r#type: "button",
                            disabled: busy,
                            aria_describedby: "setup-advanced-reset-help",
                            onclick: move |_| {
                                if pending.read().is_some() || outcome_unknown() { return; }
                                draft.write().reset_advanced();
                                issues.write().retain(|issue| !SetupField::ADVANCED.contains(&issue.field));
                                notice.set(None);
                            },
                            {t!("setup-advanced-reset")}
                        }
                        p { id: "setup-advanced-reset-help", class: "admin-setup__hint", {t!("setup-advanced-reset-help")} }
                        div { class: "admin-setup__grid",
                            for field in SetupField::ADVANCED {
                                SetupInput { key: "{field.id()}", field, draft, issues, busy }
                            }
                        }
                    }
                }
            }
            p { class: "admin-setup__hint", if initialize_and_start { {t!("setup-initialize-help")} } else { {t!("setup-save-help")} } }
            if initialize_and_start {
                label { class: "admin-setup__check",
                    input {
                        r#type: "checkbox", checked: confirm_initialize(), disabled: busy,
                        onchange: move |event| { if !busy { confirm_initialize.set(event.checked()); } },
                    }
                    {t!("setup-confirm-initialize")}
                }
            }
            div { class: "admin-setup__actions",
                button {
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| submit.call(false),
                    if pending() == Some(false) { {t!("setup-validating")} } else { {t!("setup-validate")} }
                }
                button {
                    class: "admin-btn-primary",
                    r#type: "submit",
                    disabled: busy || (initialize_and_start && !confirm_initialize()),
                    if pending() == Some(true) { {t!("setup-saving")} } else if initialize_and_start { {t!("setup-save-start")} } else { {t!("setup-save")} }
                }
            }
        }
    }
}
