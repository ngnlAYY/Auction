//! Admin 审计历史页面。
//!
//! 审计事实只读且追加式；筛选和分页请求通过 HTTP 安全 DTO 完成，浏览器不修改记录、不接触
//! 数据库模型，也不把 metadata 当作可执行内容。

use dioxus::prelude::*;
use dioxus_i18n::t;

use auction_web_shared::TruncatedText;

use crate::api::{AdminAuditFilter, AdminAuditPage, AdminAuditRecord, fetch_admin_audit};

use super::pagination::AdminPager;
use super::{AdminPanel, format_admin_time};

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
    match &*audit.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-audit-title"), detail: t!("admin-audit-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminAuditLoadError { refresh }
        },
        Some(Ok(page)) => rsx! {
            section { class: "admin-panel", aria_labelledby: "admin-audit-title",
                h1 { id: "admin-audit-title", {t!("page-audit-title")} }
                p { class: "admin-panel__note", {t!("page-audit-note")} }
                AuditFilters { filter, on_change: apply_filter }
                AuditTable { page: page.clone() }
                AdminPager {
                    page: page.page.unwrap_or(page_number()).max(1),
                    page_size: page.page_size,
                    total: audit_total(page, page_number()),
                    label: t!("page-audit-title"),
                    on_page_change: move |next_page| page_number.set(next_page),
                }
            }
        },
    }
}

#[component]
fn AdminAuditLoadError(mut refresh: Signal<u64>) -> Element {
    rsx! {
        section {
            class: "admin-panel",
            role: "alert",
            aria_labelledby: "admin-audit-error-title",
            h1 { id: "admin-audit-error-title", {t!("page-audit-title")} }
            p { class: "admin-panel__note", {t!("admin-audit-error")} }
            button {
                r#type: "button",
                onclick: move |_| refresh.set(refresh().saturating_add(1)),
                {t!("admin-audit-retry")}
            }
        }
    }
}

#[component]
fn AuditFilters(
    mut filter: Signal<AdminAuditFilter>,
    on_change: EventHandler<AdminAuditFilter>,
) -> Element {
    rsx! {
        div { class: "admin-panel__actions admin-audit-filters",
            label { class: "admin-audit-field-wide",
                {t!("admin-audit-actor")}
                input {
                    maxlength: "256",
                    value: filter().actor,
                    oninput: move |event| {
                        filter.write().actor = event.value();
                        on_change.call(filter());
                    },
                }
            }
            label { class: "admin-audit-field-actor",
                {t!("admin-audit-actor-type")}
                select {
                    value: filter().actor_type,
                    onchange: move |event| {
                        filter.write().actor_type = event.value();
                        on_change.call(filter());
                    },
                    option { value: "", {t!("admin-audit-all")} }
                    option { value: "public_user", {audit_actor_type_label("public_user")} }
                    option { value: "admin", {audit_actor_type_label("admin")} }
                    option { value: "worker", {audit_actor_type_label("worker")} }
                    option { value: "system", {audit_actor_type_label("system")} }
                }
            }
            label { class: "admin-audit-field-wide",
                {t!("admin-audit-action")}
                input {
                    maxlength: "128",
                    value: filter().action,
                    oninput: move |event| {
                        filter.write().action = event.value();
                        on_change.call(filter());
                    },
                }
            }
            label { class: "admin-audit-field-wide",
                {t!("admin-audit-entity")}
                input {
                    maxlength: "385",
                    value: filter().entity,
                    oninput: move |event| {
                        filter.write().entity = event.value();
                        on_change.call(filter());
                    },
                }
            }
            label { class: "admin-audit-field-source",
                {t!("admin-audit-source")}
                select {
                    value: filter().source,
                    onchange: move |event| {
                        filter.write().source = event.value();
                        on_change.call(filter());
                    },
                    option { value: "", {t!("admin-audit-all")} }
                    option { value: "public", {audit_source_label("public")} }
                    option { value: "admin", {audit_source_label("admin")} }
                    option { value: "worker", {audit_source_label("worker")} }
                    option { value: "system", {audit_source_label("system")} }
                }
            }
            label { class: "admin-audit-field-result",
                {t!("admin-audit-result")}
                select {
                    value: filter().result,
                    onchange: move |event| {
                        filter.write().result = event.value();
                        on_change.call(filter());
                    },
                    option { value: "", {t!("admin-audit-all")} }
                    option { value: "success", {audit_result_label("success")} }
                    option { value: "rejected", {audit_result_label("rejected")} }
                    option { value: "failure", {audit_result_label("failure")} }
                }
            }
            label { class: "admin-audit-field-wide",
                {t!("admin-audit-request-id")}
                input {
                    maxlength: "128",
                    value: filter().request_id,
                    oninput: move |event| {
                        filter.write().request_id = event.value();
                        on_change.call(filter());
                    },
                }
            }
            label { class: "admin-audit-field-date",
                {t!("admin-audit-from")}
                input {
                    r#type: "datetime-local",
                    value: filter().occurred_from,
                    oninput: move |event| {
                        filter.write().occurred_from = event.value();
                        on_change.call(filter());
                    },
                }
            }
            label { class: "admin-audit-field-date",
                {t!("admin-audit-to")}
                input {
                    r#type: "datetime-local",
                    value: filter().occurred_to,
                    oninput: move |event| {
                        filter.write().occurred_to = event.value();
                        on_change.call(filter());
                    },
                }
            }
        }
    }
}

#[component]
fn AuditTable(page: AdminAuditPage) -> Element {
    rsx! {
        section { aria_labelledby: "admin-audit-table-title",
            h2 { id: "admin-audit-table-title", {t!("admin-audit-total", total: page.total.unwrap_or(0))} }
            if page.records.is_empty() {
                p { class: "admin-panel__note", {t!("admin-audit-empty")} }
            } else {
                div { class: "public-auction-catalog admin-audit__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("admin-audit-at")} }
                                th { {t!("admin-audit-actor")} }
                                th { {t!("admin-audit-action")} }
                                th { {t!("admin-audit-entity")} }
                                th { {t!("admin-audit-source")} }
                                th { {t!("admin-audit-result")} }
                                th { {t!("admin-audit-request-id")} }
                                th { {t!("admin-audit-metadata")} }
                            }
                        }
                        tbody {
                            for record in page.records {
                                AuditRow { record }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AuditRow(record: AdminAuditRecord) -> Element {
    let actor_type = audit_actor_type_label(&record.actor_type);
    let source = audit_source_label(&record.source);
    let result = audit_result_label(&record.result);
    let request_id = record.request_id.unwrap_or_else(|| t!("value-empty"));
    let actor = format!("{} ({actor_type})", record.actor_id);
    let entity = format!("{} ({})", record.entity_type, record.entity_id);
    let metadata = metadata_text(&record.metadata);
    rsx! {
        tr { class: "admin-audit__row",
            td { "data-label": t!("admin-audit-at"), time { datetime: record.occurred_at.clone(), "{format_admin_time(&record.occurred_at)}" } }
            td { "data-label": t!("admin-audit-actor"), TruncatedText { value: actor } }
            td { "data-label": t!("admin-audit-action"), TruncatedText { value: record.action } }
            td { "data-label": t!("admin-audit-entity"), TruncatedText { value: entity } }
            td { "data-label": t!("admin-audit-source"), TruncatedText { value: source } }
            td { "data-label": t!("admin-audit-result"),
                span { class: audit_result_class(&record.result), "{result}" }
            }
            td { "data-label": t!("admin-audit-request-id"), TruncatedText { value: request_id } }
            td { "data-label": t!("admin-audit-metadata"), class: "admin-audit-metadata", TruncatedText { value: metadata, size: "wide" } }
        }
    }
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

fn audit_result_class(result: &str) -> &'static str {
    match result {
        "success" => "admin-audit__result admin-audit__result--success",
        "rejected" => "admin-audit__result admin-audit__result--rejected",
        "failure" => "admin-audit__result admin-audit__result--failure",
        _ => "admin-audit__result",
    }
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
