//! 首次部署配置步骤：分组输入、统一提交校验和短生命周期异步状态。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{
    draft::{DatabaseTarget, SetupDraft, SetupField},
    error_key,
    fields::{SetupErrors, SetupInput},
    validation::{self, SetupIssue},
};
use crate::{api::submit_first_run, surface::validation::focus_field};

const CONFIRM_INITIALIZE_ID: &str = "setup-confirm-initialize";

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
    let submit = use_callback(move |complete: bool| {
        // 回调内也检查门禁，防止键盘提交或尚未重渲染的按钮绕过 disabled。
        if pending.read().is_some() || outcome_unknown() {
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
        issues.write().clear();
        let confirmed_target = if complete && initialize_and_start {
            // 使用勾选时的快照核对本次请求，不能从提交草稿重建授权。
            let confirmed = draft.read().confirmed_database_target.clone();
            let Some(confirmed) = confirmed.filter(|target| target.matches(&request.database))
            else {
                notice.set(Some((false, "setup-error-confirm-initialize".to_owned())));
                focus_field(CONFIRM_INITIALIZE_ID);
                return;
            };
            request.confirm_initialize = true;
            Some(confirmed)
        } else {
            None
        };
        pending.set(Some(complete));
        let submitted_token = token();
        spawn(async move {
            let confirmed = confirmed_target
                .as_ref()
                .map(|target| (target.host.as_str(), target.port, target.name.as_str()));
            let result = submit_first_run(&submitted_token, &request, complete, confirmed).await;
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
                    let failure = apply_submission_failure(&mut draft.write(), &code);
                    match failure {
                        SubmissionFailure::OutcomeUnknown => {
                            outcome_unknown.set(true);
                            token.set(String::new());
                        }
                        SubmissionFailure::Authenticate => on_auth_failure.call(()),
                        SubmissionFailure::ReconfirmTarget => focus_field(CONFIRM_INITIALIZE_ID),
                        SubmissionFailure::Other => {}
                    }
                }
            }
        });
    });
    let waiting = pending.read().is_some();
    // 终态禁用字段和动作，但不能向辅助技术声称仍有请求正在等待。
    let busy = waiting || outcome_unknown();
    let displayed_target =
        initialize_and_start.then(|| validation::database_target(&draft.read())).flatten();
    let confirmation_checked = displayed_target
        .as_ref()
        .is_some_and(|target| draft.read().confirmed_database_target.as_ref() == Some(target));
    let confirmation_label = displayed_target.as_ref().map(|target| {
        t!(
            "setup-confirm-initialize",
            host: target.host.clone(),
            port: target.port.to_string(),
            name: target.name.clone()
        )
    });

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
            p { id: "setup-save-help", class: "admin-setup__hint",
                if initialize_and_start { {t!("setup-initialize-help")} } else { {t!("setup-save-help")} }
            }
            if initialize_and_start {
                div { class: "admin-setup__check",
                    input {
                        id: CONFIRM_INITIALIZE_ID,
                        name: "confirm_initialize",
                        r#type: "checkbox",
                        checked: confirmation_checked,
                        disabled: busy || displayed_target.is_none(),
                        aria_describedby: "setup-save-help",
                        onchange: move |event| {
                            if pending.read().is_some() || outcome_unknown() { return; }
                            update_confirmation(&mut draft.write(), displayed_target.as_ref(), event.checked());
                            notice.set(None);
                        },
                    }
                    label { r#for: CONFIRM_INITIALIZE_ID,
                        // 库名中的连续空格属于目标的一部分，显示时也必须保留。
                        white_space: "pre-wrap",
                        overflow_wrap: "anywhere",
                        {confirmation_label.unwrap_or_else(|| t!("setup-confirm-target-unavailable"))}
                    }
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
                    disabled: busy,
                    if pending() == Some(true) { {t!("setup-saving")} } else if initialize_and_start { {t!("setup-save-start")} } else { {t!("setup-save")} }
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SubmissionFailure {
    OutcomeUnknown,
    Authenticate,
    ReconfirmTarget,
    Other,
}

fn apply_submission_failure(draft: &mut SetupDraft, code: &str) -> SubmissionFailure {
    match code {
        "setup_outcome_unknown" => {
            draft.clear_passwords();
            SubmissionFailure::OutcomeUnknown
        }
        "setup_auth" => SubmissionFailure::Authenticate,
        "setup_confirmation_required" => {
            // 服务端在写入前已拒绝，清除旧授权并允许重新确认当前目标。
            draft.confirmed_database_target = None;
            SubmissionFailure::ReconfirmTarget
        }
        _ => SubmissionFailure::Other,
    }
}

fn update_confirmation(
    draft: &mut SetupDraft,
    displayed_target: Option<&DatabaseTarget>,
    checked: bool,
) {
    // 勾选必须对应画面上的目标。输入已改变但页面还未更新时拒绝沿用旧目标。
    draft.confirmed_database_target = displayed_target
        .filter(|target| checked && validation::database_target(draft).as_ref() == Some(*target))
        .cloned();
}

#[cfg(test)]
mod tests {
    use auction_contracts::SetupErrorResponse;

    use super::{SubmissionFailure, apply_submission_failure, update_confirmation};
    use crate::surface::setup::{
        draft::{SetupDraft, SetupField},
        validation::database_target,
    };

    #[test]
    fn setup_target_mismatch_clears_confirmation_and_allows_reconfirming() {
        let response: SetupErrorResponse = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../auction-contracts/tests/fixtures/admin/setup-confirm-target-mismatch.json"
        )))
        .unwrap();
        let mut draft = SetupDraft::default();
        let displayed = database_target(&draft).unwrap();
        update_confirmation(&mut draft, Some(&displayed), true);
        assert!(draft.confirmed_database_target.as_ref() == Some(&displayed));

        assert_eq!(
            apply_submission_failure(&mut draft, &response.code),
            SubmissionFailure::ReconfirmTarget
        );
        assert!(draft.confirmed_database_target.is_none());

        draft.set(SetupField::DatabaseName, "current_target".to_owned());
        update_confirmation(&mut draft, Some(&displayed), true);
        assert!(draft.confirmed_database_target.is_none());
        let current = database_target(&draft).unwrap();
        update_confirmation(&mut draft, Some(&current), true);
        assert!(draft.confirmed_database_target.as_ref() == Some(&current));
    }

    #[test]
    fn setup_unknown_outcome_requires_manual_verification() {
        let mut draft = SetupDraft::default();
        let displayed = database_target(&draft).unwrap();
        update_confirmation(&mut draft, Some(&displayed), true);
        assert_eq!(
            apply_submission_failure(&mut draft, "setup_outcome_unknown"),
            SubmissionFailure::OutcomeUnknown
        );
        assert!(draft.confirmed_database_target.is_none());
        assert!(draft.value(SetupField::AdminSecret).is_empty());
    }
}
