//! 凭据列表、状态操作和改名对话框入口。

use std::cmp::Ordering;

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind, Time, TruncatedText};

use crate::api::{AdminCredential, AdminCredentialPage, mutate_admin_credential};
use crate::surface::AdminActionFeedback;
use crate::surface::pending::{PendingCommands, set_if_alive};
use crate::surface::sort::{MobileSortControl, SortDirection, SortableTh, sort_rows};

use super::{
    CredentialCheckAction, credential_kind_label, credential_status_class, credential_status_label,
    credential_world_label,
};

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
                            active: sort.clone(),
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
    let mut rename_value = use_signal(|| credential.label.clone());
    let mut rename_dialog = use_signal(|| false);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<AdminActionFeedback>);
    let owner = use_context::<PendingCommands>();
    let blocked = owner.contains(&format!("credential:{}", credential.id));
    let credential_for_rename = credential.clone();
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
                aria_describedby: feedback().is_some().then_some(feedback_id.clone()),
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
                        onclick: move |_| {
                            rename_value.set(credential_for_rename.label.clone());
                            rename_dialog.set(true);
                        },
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
                                feedback.message(t!("admin-credentials-action-success")),
                            ),
                            {feedback.message(t!("admin-credentials-action-success"))}
                        }
                    }
                }
            }
        }
        if rename_dialog() {
            tr { class: "admin-credentials-dialog-row",
                td { colspan: "8",
                    Dialog {
                        title: t!("admin-credentials-rename-title"),
                        close_label: t!("admin-credentials-cancel"),
                        busy: busy(),
                        on_close: move |_| rename_dialog.set(false),
                        form {
                                onsubmit: move |event| {
                                    event.prevent_default();
                                    if busy() {
                                        return;
                                    }
                                    let credential = credential_for_rename_submit.clone();
                                    let label = rename_value();
                                    if submit_credential(owner, credential, "rename", Some(label), None, feedback) {
                                        rename_dialog.set(false);
                                    }
                                },
                                label {
                                    r#for: rename_input_id.clone(),
                                    {t!("admin-credentials-label")}
                                    input {
                                        id: rename_input_id.clone(),
                                        r#type: "text",
                                        autofocus: true,
                                        required: true,
                                        disabled: busy(),
                                        value: rename_value(),
                                        oninput: move |event| rename_value.set(event.value()),
                                    }
                                }
                                div { class: "admin-shell__dialog-actions",
                                    button {
                                        r#type: "button",
                                        class: "admin-btn-secondary",
                                        disabled: busy(),
                                        onclick: move |_| rename_dialog.set(false),
                                        {t!("admin-credentials-cancel")}
                                    }
                                    button {
                                        r#type: "submit",
                                        class: "admin-btn-primary",
                                        disabled: busy(),
                                        if busy() { {t!("admin-action-working")} } else { {t!("admin-credentials-rename-save")} }
                                    }
                                }
                            }
                        }
                }
            }
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
    feedback: Signal<Option<AdminActionFeedback>>,
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
            move |_| set_if_alive(feedback, Some(AdminActionFeedback::Success)),
            move |code| set_if_alive(feedback, Some(AdminActionFeedback::Error(code))),
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
