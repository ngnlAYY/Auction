//! Admin 审计历史页面。
//! 审计事实只读且追加式；筛选和分页请求通过 HTTP 安全 DTO 完成，浏览器不修改记录、不接触
//! 数据库模型，也不把 metadata 当作可执行内容。

use std::cmp::Ordering;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::pagination::{AdminPager as Pager, use_admin_page, use_admin_pagination};
use auction_web_shared::{AsyncState, AsyncStateKind, Time, TruncatedText};

use crate::api::{AdminAuditFilter, AdminAuditPage, AdminAuditRecord, fetch_admin_audit};

use super::admin_api_error_message;
use super::query::{AuditQuery, use_audit_query_state};
use super::sort::{MobileSortControl, SortDirection, SortableTh, sort_rows};

pub(super) fn page() -> Element {
    // 已应用的筛选与分页跟地址栏双向同步；表单草稿是提交前的瞬态，只在提交时入 URL。
    let query = use_audit_query_state();
    let query_state = query.state;
    let requested_query = use_memo(move || {
        let current = query_state.read();
        (current.page, current.filter.clone())
    });
    // 草稿初值取自地址栏恢复的已应用筛选，刷新后表单与列表保持一致；之后的编辑不落 URL。
    let filter = use_signal(move || query_state.peek().filter.clone());
    let mut refresh = use_signal(|| 0_u64);
    let pagination = use_admin_pagination("audit", &["page_size"], Some(200), i64::from(i32::MAX));
    let audit = use_admin_page(
        pagination.clone(),
        move || {
            let _ = refresh();
            requested_query()
        },
        |(page, filter), size| fetch_admin_audit(page, filter, size),
    );

    let mut apply_query = query.clone();
    let apply_filter = move |next_filter: AdminAuditFilter| {
        // 提交筛选回到第一页并保留排序；查询资源响应状态变化自动重新加载。
        apply_query.push(AuditQuery { filter: next_filter, page: 1, ..query_state() });
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
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query;
            rsx! {
            AuditTable {
                page: page.clone(),
                sort: query_state().sort,
                direction: query_state().direction,
                on_sort_change: move |(sort, direction)| {
                    let mut next = query_state();
                    next.sort = sort;
                    next.direction = direction;
                    on_sort_change.push(next);
                },
            }
            Pager {
                pagination,
                page: page.page.unwrap_or_else(|| query_state().page).max(1),
                page_size: page.page_size,
                total: page.total,
                has_more: page.has_more,
                label: t!("page-audit-title"),
                on_page_change: move |next_page| {
                    let mut next = query_state();
                    next.page = next_page;
                    on_page_change.push(next);
                },
            }
            }
        }
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
            autocomplete: "off",
            onsubmit: move |event| {
                event.prevent_default();
                submit_filter.call(filter());
            },
            p { class: "admin-audit-filter__hint", {t!("admin-audit-filter-hint")} }
            div { class: "admin-audit-filter__primary",
                label {
                    {t!("admin-audit-actor-id")}
                    input {
                        name: "actor",
                        r#type: "search",
                        autocapitalize: "none",
                        spellcheck: "false",
                        enterkeyhint: "search",
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
                        name: "entity",
                        r#type: "search",
                        autocapitalize: "none",
                        spellcheck: "false",
                        enterkeyhint: "search",
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
                        name: "action",
                        r#type: "search",
                        autocapitalize: "none",
                        spellcheck: "false",
                        enterkeyhint: "search",
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
                        name: "request_id",
                        r#type: "search",
                        autocapitalize: "none",
                        spellcheck: "false",
                        enterkeyhint: "search",
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
                            name: "actor_type",
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
                            name: "source",
                            value: filter().source,
                            onchange: move |event| {
                                filter.write().source = event.value();
                            },
                            option { value: "", {t!("admin-audit-all")} }
                            option { value: "public", {audit_source_label("public")} }
                            option { value: "admin", {audit_source_label("admin")} }
                            option { value: "cli", {audit_source_label("cli")} }
                            option { value: "worker", {audit_source_label("worker")} }
                            option { value: "system", {audit_source_label("system")} }
                        }
                    }
                    label {
                        {t!("admin-audit-result")}
                        select {
                            name: "result",
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
                            name: "occurred_from",
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
                            name: "occurred_to",
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
fn AuditTable(
    page: AdminAuditPage,
    /// 当前排序列（空串 = 服务端原始顺序，即发生时间倒序）与方向。
    sort: String,
    direction: SortDirection,
    on_sort_change: EventHandler<(String, SortDirection)>,
) -> Element {
    let empty = page.records.is_empty();
    let mut records = page.records;
    sort_audit_records(&mut records, &sort, direction);
    rsx! {
        MobileSortControl {
            columns: vec![
                ("at", t!("admin-audit-at")),
                ("actor", t!("admin-audit-actor")),
                ("action", t!("admin-audit-action")),
                ("entity", t!("admin-audit-entity")),
                ("source", t!("admin-audit-source")),
                ("result", t!("admin-audit-result")),
            ],
            sort: sort.clone(),
            direction,
            on_sort_change,
        }
        div { class: "admin-shell__table-wrap admin-audit-table-wrap",
            table { class: "admin-shell__table admin-audit-table",
                aria_label: t!("page-audit-title"),
                auction_web_shared::TableColumns { layout: "admin-audit", widths: vec![17, 12, 18, 15, 6, 6, 10, 16] }
                thead {
                    tr {
                        SortableTh {
                            column: "at",
                            label: t!("admin-audit-at"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-audit-table__time",
                        }
                        SortableTh {
                            column: "actor",
                            label: t!("admin-audit-actor"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-audit-table__actor",
                        }
                        SortableTh {
                            column: "action",
                            label: t!("admin-audit-action"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-audit-table__action",
                        }
                        SortableTh {
                            column: "entity",
                            label: t!("admin-audit-entity"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-audit-table__entity",
                        }
                        SortableTh {
                            column: "source",
                            label: t!("admin-audit-source"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-audit-table__source",
                        }
                        SortableTh {
                            column: "result",
                            label: t!("admin-audit-result"),
                            active: sort,
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-audit-table__result",
                        }
                        th { class: "admin-audit-table__request", scope: "col", {t!("admin-audit-request-id")} }
                        th { class: "admin-audit-table__metadata", scope: "col", {t!("admin-audit-metadata")} }
                    }
                }
                tbody {
                    for record in records {
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
        "cli" => t!("admin-audit-source-cli"),
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

/// 对当前页审计记录排序；`column` 为空或不在允许集合时保持服务端原始顺序（发生时间倒序）。
fn sort_audit_records(records: &mut [AdminAuditRecord], column: &str, direction: SortDirection) {
    sort_rows(records, column, direction, |left, right, column| match column {
        "at" => left.occurred_at.cmp(&right.occurred_at),
        "actor" => left.actor_id.cmp(&right.actor_id),
        "action" => left.action.cmp(&right.action),
        "entity" => left.entity_id.cmp(&right.entity_id),
        "source" => left.source.cmp(&right.source),
        "result" => left.result.cmp(&right.result),
        _ => Ordering::Equal,
    });
}
