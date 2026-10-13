//! 凭据列表、状态操作和改名对话框入口。

use std::cmp::Ordering;

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;
use uuid::Uuid;

use auction_web_shared::{AsyncState, AsyncStateKind, Pager, Time, TruncatedText};

use crate::api::{AdminCredential, AdminCredentialPage, AdminSession, mutate_admin_credential};
use crate::app::surface::sort::{SortDirection, SortableTh, directed};
use crate::app::surface::{AdminActionFeedback, run_admin_feedback_action, stable_command_id};

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
    reload_generation: Signal<u64>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    let session = use_context::<AdminSession>();
    let is_empty = page.credentials.is_empty();
    // 服务端不支持排序参数，浏览器只对当前页排序；未选择排序列时保持原始顺序。
    let mut credentials = page.credentials.clone();
    sort_credentials(&mut credentials, &sort, direction);
    rsx! {
        p { class: "admin-credentials__feedback", {t!("admin-credentials-check-hint")} }
        div { class: "admin-credentials-table-wrap",
            table { class: "admin-credentials-table",
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
                        th { {t!("admin-credentials-last-used")} }
                        th { {t!("admin-credentials-imported-by")} }
                        th { {t!("admin-credentials-actions")} }
                    }
                }
                tbody {
                    for credential in credentials {
                        CredentialRow {
                            key: "{credential.id}",
                            credential,
                            imported_by: session.actor.clone(),
                            reload_generation,
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
    imported_by: String,
    reload_generation: Signal<u64>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    let mut rename_value = use_signal(|| credential.label.clone());
    let mut rename_dialog = use_signal(|| false);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
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
                h3 { class: "admin-visually-hidden", {credential.label.clone()} }
                TruncatedText { value: credential.label.clone() }
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
            td { "data-label": t!("admin-credentials-imported-by"),
                TruncatedText { value: imported_by }
            }
            td {
                "data-label": t!("admin-credentials-actions"),
                class: "admin-credentials__row-actions",
                aria_describedby: feedback().is_some().then_some(feedback_id.clone()),
                button {
                    r#type: "button",
                    disabled: busy() || credential.status == "active",
                    onclick: move |_| {
                        let credential = credential_for_enable.clone();
                        let status = "active";
                        let command_id = stable_command_id(
                            command_state,
                            format!(
                                "{}:set_status:{}:{status}",
                                credential.id, credential.state_version
                            ),
                        );
                        run_admin_feedback_action(
                            busy,
                            feedback,
                            async move {
                                mutate_admin_credential(
                                    command_id,
                                    &credential,
                                    "set_status",
                                    None,
                                    Some(status.to_owned()),
                                )
                                .await
                                .map(|_| ())
                            },
                            move |()| {
                                command_state.set(None);
                                reload_generation.set(reload_generation().saturating_add(1));
                            },
                        );
                    },
                    {t!("admin-credentials-action-enable")}
                }
                button {
                    r#type: "button",
                    disabled: busy() || credential.status == "disabled",
                    onclick: move |_| {
                        let credential = credential_for_disable.clone();
                        let status = "disabled";
                        let command_id = stable_command_id(
                            command_state,
                            format!(
                                "{}:set_status:{}:{status}",
                                credential.id, credential.state_version
                            ),
                        );
                        run_admin_feedback_action(
                            busy,
                            feedback,
                            async move {
                                mutate_admin_credential(
                                    command_id,
                                    &credential,
                                    "set_status",
                                    None,
                                    Some(status.to_owned()),
                                )
                                .await
                                .map(|_| ())
                            },
                            move |()| {
                                command_state.set(None);
                                reload_generation.set(reload_generation().saturating_add(1));
                            },
                        );
                    },
                    {t!("admin-credentials-action-disable")}
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        rename_value.set(credential_for_rename.label.clone());
                        rename_dialog.set(true);
                    },
                    {t!("admin-credentials-action-rename-open")}
                }
                button {
                    r#type: "button",
                    class: "admin-btn-danger",
                    disabled: busy(),
                    onclick: move |_| on_delete.call(credential_for_delete.clone()),
                    {t!("admin-credentials-action-delete")}
                }
                CredentialCheckAction { credential: credential.clone(), busy }
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
        if rename_dialog() {
            tr { class: "admin-credentials-dialog-row",
                td { colspan: "8",
                    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管。
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
                                    let command_id = stable_command_id(
                                        command_state,
                                        format!(
                                            "{}:rename:{}:{label}",
                                            credential.id, credential.state_version
                                        ),
                                    );
                                    let mut rename_dialog = rename_dialog;
                                    run_admin_feedback_action(
                                        busy,
                                        feedback,
                                        async move {
                                            let result = mutate_admin_credential(
                                                command_id,
                                                &credential,
                                                "rename",
                                                Some(label),
                                                None,
                                            )
                                            .await
                                            .map(|_| ());
                                            if result.is_ok() {
                                                rename_dialog.set(false);
                                            }
                                            result
                                        },
                                        move |()| {
                                            command_state.set(None);
                                            reload_generation
                                                .set(reload_generation().saturating_add(1));
                                        },
                                    );
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

#[component]
pub fn CredentialPager(page: AdminCredentialPage, on_page_change: EventHandler<i64>) -> Element {
    rsx! {
        Pager {
            page: page.page,
            page_size: page.page_size,
            total: Some(page.total),
            label: t!("page-credentials-title"),
            previous_label: t!("admin-pagination-previous"),
            status: move |(page, pages): (i64, i64)| {
                t!("admin-pagination-status", page: page, pages: pages)
            },
            next_label: t!("admin-pagination-next"),
            jump_label: t!("admin-pagination-input"),
            jump_button_label: t!("admin-pagination-go"),
            on_page_change,
        }
    }
}

/// 对当前页凭据行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
/// 轮换时间缺省（None）在升序时排在最前；checked 保留为旧 URL 排序键。
fn sort_credentials(credentials: &mut [AdminCredential], column: &str, direction: SortDirection) {
    if column.is_empty() {
        return;
    }
    credentials.sort_by(|left, right| {
        directed(
            match column {
                "label" => left.label.cmp(&right.label),
                "world" => left.world.cmp(&right.world),
                "kind" => left.kind.cmp(&right.kind),
                "status" => left.status.cmp(&right.status),
                "checked" => left.rotated_at.cmp(&right.rotated_at),
                _ => Ordering::Equal,
            },
            direction,
        )
    });
}
