//! 凭据列表、状态操作和改名对话框入口。

use std::cmp::Ordering;

use auction_web_shared::a11y::focus_element_by_id;
use auction_web_shared::{AsyncState, AsyncStateKind, Time, TruncatedText};
use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::field_validation::{
    CredentialField, CredentialFieldError, CredentialFieldErrors, credential_field_issue_message,
    use_credential_fields,
};
use super::labels::CredentialFeedback;
use super::{
    CredentialCheckAction, credential_kind_label, credential_status_class, credential_status_label,
    credential_world_label,
};
use crate::api::{AdminCredential, AdminCredentialPage, mutate_admin_credential};
use crate::surface::config::{FieldLiveRegion, use_field_announcer};
use crate::surface::pending::{PendingCommands, set_if_alive};
use crate::surface::sort::{MobileSortControl, SortDirection, SortableTh, sort_rows};

#[component]
pub fn CredentialTable(
    page: AdminCredentialPage,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    on_sort_change: EventHandler<(String, SortDirection)>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    let is_empty = page.credentials.is_empty();
    let mut credentials = page.credentials;
    sort_credentials(&mut credentials, &sort, direction);
    rsx! {
        p { class: "admin-credentials__feedback", {t!("admin-credentials-check-hint")} }
        MobileSortControl {
            columns: vec![
                ("label", t!("admin-credentials-label")),
                ("world", t!("admin-credentials-world")),
                ("kind", t!("admin-credentials-kind")),
                ("status", t!("admin-credentials-status")),
                ("checked", t!("admin-credentials-rotated-at")),
            ],
            sort: sort.clone(),
            direction,
            on_sort_change,
        }
        div { class: "admin-credentials-table-wrap",
            table { class: "admin-credentials-table",
                aria_label: t!("page-credentials-title"),
                // 列序：名称、世界、类型、状态、轮换时间、使用时间、导入者、操作；五个操作按钮允许换行。
                auction_web_shared::TableColumns { layout: "admin-credentials", widths: vec![14, 6, 10, 6, 14, 14, 6, 30] }
                thead {
                    tr {
                        SortableTh {
                            column: "label",
                            label: t!("admin-credentials-label"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "world",
                            label: t!("admin-credentials-world"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "kind",
                            label: t!("admin-credentials-kind"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "status",
                            label: t!("admin-credentials-status"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "checked",
                            // 保留旧 URL 排序键 checked，但 rotated_at 不能冒充真实检查时间。
                            label: t!("admin-credentials-rotated-at"),
                            active: sort,
                            direction,
                            on_sort: on_sort_change,
                        }
                        th { scope: "col", {t!("admin-credentials-last-used")} }
                        th { scope: "col", {t!("admin-credentials-imported-by")} }
                        th { scope: "col", {t!("admin-credentials-actions")} }
                    }
                }
                tbody {
                    for credential in credentials {
                        CredentialRow {
                            key: "{credential.id}",
                            credential,
                            on_delete,
                        }
                    }
                }
            }
        }
        if is_empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-credentials-empty"),
                detail: "",
            }
        }
    }
}

#[component]
pub fn CredentialRow(
    credential: AdminCredential,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    let mut rename_dialog = use_signal(|| false);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<CredentialFeedback>);
    let owner = use_context::<PendingCommands>();
    let blocked = owner.contains(&format!("credential:{}", credential.id));
    let credential_for_rename_submit = credential.clone();
    let credential_for_enable = credential.clone();
    let credential_for_disable = credential.clone();
    let credential_for_delete = credential.clone();
    let feedback_id = format!("admin-credential-feedback-{}", credential.id);
    let rename_input_id = format!("label-{}", credential.id);
    rsx! {
        tr { class: "admin-credentials__row",
            td { "data-label": t!("admin-credentials-label"),
                TruncatedText { value: credential.label.clone() }
                if let Some(player_name) = credential.player_name.clone() {
                    span { title: t!("admin-credentials-player-name-help"),
                        {t!("admin-credentials-player-name")}
                        TruncatedText { value: player_name, size: "compact" }
                    }
                }
            }
            td { "data-label": t!("admin-credentials-world"), TruncatedText { value: credential_world_label(&credential.world), size: "compact" } }
            td { "data-label": t!("admin-credentials-kind"), {credential_kind_label(&credential.kind)} }
            td { "data-label": t!("admin-credentials-status"),
                span {
                    class: credential_status_class(&credential.status),
                    {credential_status_label(&credential.status)}
                }
            }
            td { "data-label": t!("admin-credentials-rotated-at"),
                if let Some(rotated_at) = credential.rotated_at.clone() {
                    Time { value: rotated_at }
                } else {
                    {t!("value-empty")}
                }
            }
            td { "data-label": t!("admin-credentials-last-used"), {t!("value-empty")} }
            // 安全 DTO 不含导入者，当前登录 actor 不能代替历史操作事实。
            td { "data-label": t!("admin-credentials-imported-by"), {t!("value-empty")} }
            td {
                "data-label": t!("admin-credentials-actions"),
                aria_describedby: feedback().is_some().then_some(feedback_id),
                div { class: "admin-credentials__row-actions",
                    button {
                        r#type: "button",
                        disabled: busy() || blocked || credential.status == "active",
                        onclick: move |_| {
                            let credential = credential_for_enable.clone();
                            if busy() { return; }
                            submit_credential(owner, credential, "set_status", None, Some("active".to_owned()), feedback);
                        },
                        {t!("admin-credentials-action-enable")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy() || blocked || credential.status == "disabled",
                        onclick: move |_| {
                            let credential = credential_for_disable.clone();
                            if busy() { return; }
                            submit_credential(owner, credential, "set_status", None, Some("disabled".to_owned()), feedback);
                        },
                        {t!("admin-credentials-action-disable")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy() || blocked,
                        onclick: move |_| rename_dialog.set(true),
                        {t!("admin-credentials-action-rename-open")}
                    }
                    button {
                        r#type: "button",
                        class: "admin-btn-danger",
                        disabled: busy() || blocked,
                        onclick: move |_| on_delete.call(credential_for_delete.clone()),
                        {t!("admin-credentials-action-delete")}
                    }
                    CredentialCheckAction { credential: credential.clone(), busy, blocked }
                    if let Some(feedback) = feedback() {
                        span {
                            id: feedback_id.clone(),
                            class: if feedback.is_success() {
                                "admin-credentials__feedback admin-credentials__feedback--success"
                            } else {
                                "admin-credentials__feedback admin-credentials__feedback--error"
                            },
                            role: if feedback.is_success() { "status" } else { "alert" },
                            aria_label: format!(
                                "{}: {}",
                                credential.label,
                                feedback.message(),
                            ),
                            {feedback.message()}
                        }
                    }
                }
            }
        }
        if rename_dialog() {
            tr { class: "admin-credentials-dialog-row",
                td { colspan: "8",
                    CredentialRenameDialog {
                        initial_label: credential.label.clone(),
                        input_id: rename_input_id,
                        busy: busy(),
                        on_close: move |_| rename_dialog.set(false),
                        on_save: move |label| {
                            let credential = credential_for_rename_submit.clone();
                            if submit_credential(owner, credential, "rename", Some(label), None, feedback) {
                                rename_dialog.set(false);
                            }
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn CredentialRenameDialog(
    initial_label: String,
    input_id: String,
    busy: bool,
    on_close: EventHandler<()>,
    on_save: EventHandler<String>,
) -> Element {
    let label_field = CredentialField::Label;
    let mut rename_value = use_signal(|| initial_label);
    let announcer = use_field_announcer();
    let fields = use_credential_fields(announcer);
    let issue = fields.error(label_field);
    let focus_id = input_id.clone();
    let error_id = format!("{input_id}-error");
    rsx! {
        Dialog {
            title: t!("admin-credentials-rename-title"),
            close_label: t!("admin-credentials-cancel"),
            busy,
            on_close,
            form {
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy {
                        return;
                    }
                    let label = rename_value();
                    let issue = label_field.issue(&label);
                    let mut errors = CredentialFieldErrors::default();
                    errors.set(label_field, issue);
                    fields.show(errors);
                    if let Some(issue) = issue {
                        focus_element_by_id(&focus_id);
                        announcer.announce(credential_field_issue_message(issue));
                        return;
                    }
                    on_save.call(label);
                },
                label {
                    r#for: input_id.clone(),
                    {t!("admin-credentials-label")}
                    input {
                        id: input_id,
                        r#type: "text",
                        autofocus: true,
                        required: true,
                        disabled: busy,
                        aria_invalid: issue.is_some().to_string(),
                        aria_describedby: issue.is_some().then_some(error_id),
                        value: rename_value(),
                        oninput: move |event| {
                            let value = event.value();
                            fields.edited(label_field, &value);
                            rename_value.set(value);
                        },
                        onblur: move |_| fields.blurred(label_field, &rename_value.peek()),
                    }
                }
                CredentialFieldError { id: error_id.clone(), issue }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        class: "admin-btn-secondary",
                        disabled: busy,
                        onclick: move |_| on_close.call(()),
                        {t!("admin-credentials-cancel")}
                    }
                    button {
                        r#type: "submit",
                        class: "admin-btn-primary",
                        disabled: busy,
                        if busy { {t!("admin-action-working")} } else { {t!("admin-credentials-rename-save")} }
                    }
                }
            }
            FieldLiveRegion { announcer }
        }
    }
}

/// 只把当前已提交的安全凭据投影与候选值交给页宿主，不持有 Cookie 或结果。
fn submit_credential(
    owner: PendingCommands,
    credential: AdminCredential,
    action: &'static str,
    label: Option<String>,
    status: Option<String>,
    feedback: Signal<Option<CredentialFeedback>>,
) -> bool {
    let action_label = if action == "rename" {
        t!("admin-credentials-action-rename-open")
    } else if status.as_deref() == Some("active") {
        t!("admin-credentials-action-enable")
    } else {
        t!("admin-credentials-action-disable")
    };
    let accepted =
        owner.submit(
            format!("credential:{}", credential.id),
            action_label,
            move |command_id| {
                let credential = credential.clone();
                let label = label.clone();
                let status = status.clone();
                async move {
                    mutate_admin_credential(command_id, &credential, action, label, status).await
                }
            },
            move |result| {
                set_if_alive(
                    feedback,
                    Some(CredentialFeedback::Success { restart_required: result.restart_required }),
                );
            },
            move |code| set_if_alive(feedback, Some(CredentialFeedback::Error(code))),
        );
    if accepted {
        set_if_alive(feedback, None);
    }
    accepted
}
/// 对当前页凭据行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
/// 轮换时间缺省（None）在升序时排在最前；checked 保留为旧 URL 排序键。
fn sort_credentials(credentials: &mut [AdminCredential], column: &str, direction: SortDirection) {
    sort_rows(credentials, column, direction, |left, right, column| match column {
        "label" => left.label.cmp(&right.label),
        "world" => left.world.cmp(&right.world),
        "kind" => left.kind.cmp(&right.kind),
        "status" => left.status.cmp(&right.status),
        "checked" => left.rotated_at.cmp(&right.rotated_at),
        _ => Ordering::Equal,
    });
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::core::{AttributeValue, Mutation};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::CredentialRenameDialog;
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::config::test_form::{dispatch, listener};

    fn rename_dialog() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        let submitted = use_context::<Rc<RefCell<Vec<String>>>>();
        let mut open = use_signal(|| true);
        rsx! {
            if open() {
                CredentialRenameDialog {
                    initial_label: "旧标签".to_owned(),
                    input_id: "rename-label".to_owned(),
                    busy: false,
                    on_close: move |_| open.set(false),
                    on_save: move |label| {
                        submitted.borrow_mut().push(label);
                        open.set(false);
                    },
                }
            }
        }
    }

    #[test]
    fn rename_rejects_invalid_labels_before_submitting_and_accepts_corrections() {
        let submitted = Rc::new(RefCell::new(Vec::<String>::new()));
        let mut dom = VirtualDom::new(rename_dialog).with_root_context(submitted.clone());
        let initial = dom.rebuild_to_vec();
        let submit = listener(&initial.edits, "submit");
        let input = listener(&initial.edits, "input");

        dispatch(&dom, input, "input", " ");
        dispatch(&dom, submit, "submit", "");
        assert!(submitted.borrow().is_empty());
        let rejected = dom.render_immediate_to_vec().edits;
        assert!(rejected.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: "aria-invalid", value: AttributeValue::Text(value), .. }
                if *id == input && value == "true"
        )));
        assert!(rejected.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: "aria-describedby", value: AttributeValue::Text(value), .. }
                if *id == input && value == "rename-label-error"
        )));
        assert!(rejected.iter().any(|edit| matches!(edit,
            Mutation::CreateTextNode { value, .. } if value == "请填写此字段。"
        )));

        dispatch(&dom, input, "input", "仍含空白 ");
        let still_invalid = dom.render_immediate_to_vec().edits;
        assert!(!still_invalid.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: "aria-invalid", value: AttributeValue::Text(value), .. }
                if *id == input && value == "false"
        )));
        dispatch(&dom, submit, "submit", "");
        assert!(submitted.borrow().is_empty());

        dispatch(&dom, input, "input", "新标签");
        let corrected = dom.render_immediate_to_vec().edits;
        assert!(corrected.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { id, name: "aria-invalid", value: AttributeValue::Text(value), .. }
                if *id == input && value == "false"
        )));
        dispatch(&dom, submit, "submit", "");
        assert_eq!(submitted.borrow().as_slice(), ["新标签"]);
    }
}
