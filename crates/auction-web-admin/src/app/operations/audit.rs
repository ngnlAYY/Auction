//! Admin 审计历史页面。
//!
//! 审计事实只读且追加式；筛选和分页请求通过 HTTP 安全 DTO 完成，浏览器不修改记录、不接触
//! 数据库模型，也不把 metadata 当作可执行内容。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind, Pager, Time, TruncatedText};

use crate::api::{AdminAuditFilter, AdminAuditPage, AdminAuditRecord, fetch_admin_audit};

use super::admin_api_error_message;

pub(super) fn page() -> Element {
    let filter = use_signal(AdminAuditFilter::default);
    let mut active_filter = use_signal(AdminAuditFilter::default);
    let mut page_number = use_signal(|| 1_i64);
    let mut refresh = use_signal(|| 0_u64);
    let audit = use_resource(move || {
        let _ = refresh();
        fetch_admin_audit(page_number(), active_filter())
    });

    let apply_filter = move |next_filter: AdminAuditFilter| {
        active_filter.set(next_filter);
        page_number.set(1);
        refresh.set(refresh().saturating_add(1));
    };
    let content = match &*audit.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-audit-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-audit-retry"),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
        },
        Some(Ok(page)) => rsx! {
            AuditTable { page: page.clone() }
            Pager {
                page: page.page.unwrap_or(page_number()).max(1),
                page_size: page.page_size,
                total: Some(audit_total(page, page_number())),
                label: t!("page-audit-title"),
                previous_label: t!("admin-pagination-previous"),
                status: move |(page, pages): (i64, i64)| {
                    t!("admin-pagination-status", page: page, pages: pages)
                },
                next_label: t!("admin-pagination-next"),
                jump_label: t!("admin-pagination-input"),
                jump_button_label: t!("admin-pagination-go"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
        },
    };
    rsx! {
        section { class: "admin-audit-page", aria_labelledby: "admin-audit-title",
            h1 { id: "admin-audit-title", {t!("page-audit-title")} }
            AuditFilters { filter, on_submit: apply_filter }
            {content}
        }
    }
}

#[component]
fn AuditFilters(
    mut filter: Signal<AdminAuditFilter>,
    on_submit: EventHandler<AdminAuditFilter>,
) -> Element {
    let submit_filter = on_submit;
    let clear_filter = on_submit;
    rsx! {
        form {
            class: "admin-audit-filter",
            onsubmit: move |event| {
                event.prevent_default();
                submit_filter.call(filter());
            },
            p { class: "admin-audit-filter__hint", {t!("admin-audit-filter-hint")} }
            div { class: "admin-audit-filter__primary",
                label {
                    {t!("admin-audit-actor-id")}
                    input {
                        maxlength: "256",
                        value: filter().actor,
                        oninput: move |event| {
                            filter.write().actor = event.value();
                        },
                    }
                }
                label {
                    {t!("admin-audit-entity-identity")}
                    input {
                        maxlength: "385",
                        value: filter().entity,
                        oninput: move |event| {
                            filter.write().entity = event.value();
                        },
                    }
                }
                label {
                    {t!("admin-audit-action")}
                    input {
                        maxlength: "128",
                        value: filter().action,
                        oninput: move |event| {
                            filter.write().action = event.value();
                        },
                    }
                }
                label {
                    {t!("admin-audit-request-id")}
                    input {
                        maxlength: "128",
                        value: filter().request_id,
                        oninput: move |event| {
                            filter.write().request_id = event.value();
                        },
                    }
                }
            }
            details {
                class: "admin-audit-filter__advanced",
                open: audit_advanced_filter_active(&filter()),
                summary { {t!("admin-audit-advanced-filters")} }
                div { class: "admin-audit-filter__advanced-grid",
                    label {
                        {t!("admin-audit-actor-type")}
                        select {
                            value: filter().actor_type,
                            onchange: move |event| {
                                filter.write().actor_type = event.value();
                            },
                            option { value: "", {t!("admin-audit-all")} }
                            option { value: "public_user", {audit_actor_type_label("public_user")} }
                            option { value: "admin", {audit_actor_type_label("admin")} }
                            option { value: "worker", {audit_actor_type_label("worker")} }
                            option { value: "system", {audit_actor_type_label("system")} }
                        }
                    }
                    label {
                        {t!("admin-audit-source")}
                        select {
                            value: filter().source,
                            onchange: move |event| {
                                filter.write().source = event.value();
                            },
                            option { value: "", {t!("admin-audit-all")} }
                            option { value: "public", {audit_source_label("public")} }
                            option { value: "admin", {audit_source_label("admin")} }
                            option { value: "worker", {audit_source_label("worker")} }
                            option { value: "system", {audit_source_label("system")} }
                        }
                    }
                    label {
                        {t!("admin-audit-result")}
                        select {
                            value: filter().result,
                            onchange: move |event| {
                                filter.write().result = event.value();
                            },
                            option { value: "", {t!("admin-audit-all")} }
                            option { value: "success", {audit_result_label("success")} }
                            option { value: "rejected", {audit_result_label("rejected")} }
                            option { value: "failure", {audit_result_label("failure")} }
                        }
                    }
                    label {
                        {t!("admin-audit-from")}
                        input {
                            r#type: "datetime-local",
                            value: filter().occurred_from,
                            oninput: move |event| {
                                filter.write().occurred_from = event.value();
                            },
                        }
                    }
                    label {
                        {t!("admin-audit-to")}
                        input {
                            r#type: "datetime-local",
                            value: filter().occurred_to,
                            oninput: move |event| {
                                filter.write().occurred_to = event.value();
                            },
                        }
                    }
                }
            }
            div { class: "admin-audit-filter__actions",
                button {
                    class: "admin-btn-primary",
                    r#type: "submit",
                    {t!("admin-audit-load")}
                }
                button {
                    r#type: "button",
                    onclick: move |_| {
                        let empty = AdminAuditFilter::default();
                        filter.set(empty.clone());
                        clear_filter.call(empty);
                    },
                    {t!("admin-audit-clear")}
                }
            }
        }
    }
}

#[component]
fn AuditTable(page: AdminAuditPage) -> Element {
    let empty = page.records.is_empty();
    rsx! {
        div { class: "admin-shell__table-wrap admin-audit-table-wrap",
            table { class: "admin-shell__table admin-audit-table ui-table--responsive",
                caption { class: "admin-visually-hidden", {t!("page-audit-title")} }
                thead {
                    tr {
                        th { class: "admin-audit-table__time", scope: "col", {t!("admin-audit-at")} }
                        th { class: "admin-audit-table__actor", scope: "col", {t!("admin-audit-actor")} }
                        th { class: "admin-audit-table__action", scope: "col", {t!("admin-audit-action")} }
                        th { class: "admin-audit-table__entity", scope: "col", {t!("admin-audit-entity")} }
                        th { class: "admin-audit-table__source", scope: "col", {t!("admin-audit-source")} }
                        th { class: "admin-audit-table__result", scope: "col", {t!("admin-audit-result")} }
                        th { class: "admin-audit-table__request", scope: "col", {t!("admin-audit-request-id")} }
                        th { class: "admin-audit-table__metadata", scope: "col", {t!("admin-audit-metadata")} }
                    }
                }
                tbody {
                    for record in page.records {
                        AuditRow { record }
                    }
                }
            }
        }
        if empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-audit-empty"),
                detail: "",
            }
        }
    }
}

#[component]
fn AuditRow(record: AdminAuditRecord) -> Element {
    let actor_type = audit_actor_type_label(&record.actor_type);
    let source = audit_source_label(&record.source);
    let result = audit_result_label(&record.result);
    let result_class = match record.result.as_str() {
        "success" => "admin-audit-result admin-audit-result--success",
        "rejected" => "admin-audit-result admin-audit-result--rejected",
        "failure" => "admin-audit-result admin-audit-result--failure",
        _ => "admin-audit-result admin-audit-result--neutral",
    };
    let request_id = record.request_id.unwrap_or_else(|| t!("value-empty"));
    let metadata = metadata_text(&record.metadata);
    let actor_identity = format!("{}({actor_type})", record.actor_id);
    let entity_identity = format!("{}({})", record.entity_id, record.entity_type);
    rsx! {
        tr { class: "admin-audit__row",
            td {
                class: "admin-audit-table__time",
                "data-label": t!("admin-audit-at"),
                Time { value: record.occurred_at.clone() }
            }
            td {
                class: "admin-audit-table__actor",
                "data-label": t!("admin-audit-actor"),
                TruncatedText {
                    value: actor_identity,
                    class_name: "admin-audit-table__value admin-audit-identity",
                }
            }
            td {
                class: "admin-audit-table__action",
                "data-label": t!("admin-audit-action"),
                TruncatedText {
                    value: record.action,
                    class_name: "admin-audit-table__value admin-audit-action",
                }
            }
            td {
                class: "admin-audit-table__entity",
                "data-label": t!("admin-audit-entity"),
                TruncatedText {
                    value: entity_identity,
                    class_name: "admin-audit-table__value admin-audit-identity admin-audit-code",
                }
            }
            td {
                class: "admin-audit-table__source",
                "data-label": t!("admin-audit-source"),
                span { class: "admin-audit-source", "{source}" }
            }
            td {
                class: "admin-audit-table__result",
                "data-label": t!("admin-audit-result"),
                span { class: result_class, "{result}" }
            }
            td {
                class: "admin-audit-table__request",
                "data-label": t!("admin-audit-request-id"),
                TruncatedText {
                    value: request_id,
                    class_name: "admin-audit-table__value admin-audit-code",
                }
            }
            td {
                class: "admin-audit-table__metadata admin-audit-metadata",
                "data-label": t!("admin-audit-metadata"),
                TruncatedText {
                    value: metadata,
                    size: "wide",
                    class_name: "admin-audit-table__value admin-audit-code",
                }
            }
        }
    }
}

fn audit_advanced_filter_active(filter: &AdminAuditFilter) -> bool {
    !filter.actor_type.is_empty()
        || !filter.source.is_empty()
        || !filter.result.is_empty()
        || !filter.occurred_from.is_empty()
        || !filter.occurred_to.is_empty()
}

fn metadata_text(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
}

fn audit_total(page: &AdminAuditPage, requested_page: i64) -> i64 {
    let page_size = page.page_size.max(1);
    let current_page = page.page.unwrap_or(requested_page).max(1);
    page.total.unwrap_or_else(|| {
        let loaded = current_page.saturating_mul(page_size);
        if page.has_more { loaded.saturating_add(1) } else { loaded }
    })
}

fn audit_actor_type_label(value: &str) -> String {
    match value {
        "public_user" => t!("admin-audit-actor-public-user"),
        "admin" => t!("admin-audit-actor-admin"),
        "worker" => t!("admin-audit-actor-worker"),
        "system" => t!("admin-audit-actor-system"),
        _ => value.to_owned(),
    }
}

fn audit_source_label(value: &str) -> String {
    match value {
        "public" => t!("admin-audit-source-public"),
        "admin" => t!("admin-audit-source-admin"),
        "worker" => t!("admin-audit-source-worker"),
        "system" => t!("admin-audit-source-system"),
        _ => value.to_owned(),
    }
}

fn audit_result_label(value: &str) -> String {
    match value {
        "success" => t!("admin-audit-result-success"),
        "rejected" => t!("admin-audit-result-rejected"),
        "failure" => t!("admin-audit-result-failure"),
        _ => value.to_owned(),
    }
}
