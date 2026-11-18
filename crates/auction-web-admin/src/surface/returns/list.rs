//! 结算退件列表页：按范围、状态、证据分类、义务种类与世界筛选，分页列出案件，进入详情处理。
//! 筛选与页码同步到地址栏；列表只读，所有处理动作在详情页确认后提交。

use auction_contracts::{
    ADMIN_SETTLEMENT_RETURN_MATCH_STATES, ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS,
    ADMIN_SETTLEMENT_RETURN_STATUSES,
};
use auction_web_shared::{AsyncState, AsyncStateKind, QueryState, Time, TruncatedText, t};
use dioxus::prelude::*;

use super::labels::{
    kind_key, label, location_key, match_key, reason_key, shows_asset_location, status_key,
};
use crate::api::{
    SettlementReturnPage, SettlementReturnSummary, fetch_settlement_returns, valid_uuid,
};
use crate::surface::admin_api_error_message;
use crate::surface::admin_world_label;
use crate::surface::pagination::{AdminPager, use_admin_page, use_admin_pagination};
use crate::surface::pending::use_pending_commands;
use crate::surface::query::{ReturnsQuery, use_returns_query_state};
use crate::surface::router::Route;

pub(crate) fn page() -> Element {
    let query = use_returns_query_state();
    let query_state = query.state;
    let mut refresh = use_signal(|| 0_u64);
    use_pending_commands(refresh, |entity| entity.starts_with("settlement-return:"));
    let mut lookup = use_signal(String::new);
    let mut lookup_invalid = use_signal(|| false);
    let navigator = navigator();
    let open_lookup = move |event: FormEvent| {
        event.prevent_default();
        let target = lookup().trim().to_owned();
        if !valid_uuid(&target) {
            lookup_invalid.set(true);
            return;
        }
        lookup_invalid.set(false);
        let _ = navigator.push(format!("/settlement-returns/{target}"));
    };
    let pagination = use_admin_pagination("settlement-returns", &["page_size"], Some(100), 10_000);
    let returns = use_admin_page(
        pagination.clone(),
        move || {
            let _ = refresh();
            query_state()
        },
        |query: ReturnsQuery, size| fetch_settlement_returns(query.filters(), query.page, size),
    );
    let content = match &*returns.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("settlement-return-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-retry"),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
        },
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            rsx! {
                ReturnsTable { page: page.clone() }
                AdminPager {
                    pagination,
                    page: page.pagination.page,
                    page_size: page.pagination.page_size,
                    total: Some(page.pagination.total),
                    label: t!("settlement-return-pagination"),
                    on_page_change: move |next_page| {
                        on_page_change.push(ReturnsQuery { page: next_page, ..query_state() });
                    },
                }
            }
        }
    };
    let current = query_state();
    let mut scope_query = query.clone();
    let mut status_query = query.clone();
    let mut match_query = query.clone();
    let mut kind_query = query.clone();
    let mut world_query = query;

    rsx! {
        section { class: "admin-settlement-returns", aria_labelledby: "settlement-returns-title",
            h1 { id: "settlement-returns-title", {t!("page-settlement-returns-title")} }
            p { class: "admin-panel__note", {t!("settlement-return-intro")} }
            form { class: "admin-settlement-returns__lookup", onsubmit: open_lookup,
                label { r#for: "settlement-return-lookup-id",
                    span { {t!("settlement-return-lookup-label")} }
                    input {
                        id: "settlement-return-lookup-id",
                        r#type: "text",
                        value: lookup(),
                        oninput: move |event| {
                            lookup.set(event.value());
                            lookup_invalid.set(false);
                        },
                    }
                }
                button { r#type: "submit", {t!("settlement-return-lookup-open")} }
                if lookup_invalid() {
                    p { class: "action-feedback action-feedback--error", role: "alert",
                        {t!("settlement-return-lookup-invalid")}
                    }
                }
            }
            div { class: "admin-settlement-returns__filters",
                label {
                    {t!("settlement-return-filter-scope")}
                    select {
                        value: if current.all { "all" } else { "open" },
                        onchange: move |event| {
                            push_filter(&mut scope_query, |next| next.all = event.value() == "all");
                        },
                        option { value: "open", {t!("settlement-return-scope-open")} }
                        option { value: "all", {t!("settlement-return-scope-all")} }
                    }
                }
                label {
                    {t!("settlement-return-filter-status")}
                    select {
                        value: current.status.clone(),
                        onchange: move |event| {
                            push_filter(&mut status_query, |next| {
                                next.status =
                                    closed_choice(&ADMIN_SETTLEMENT_RETURN_STATUSES, &event.value());
                            });
                        },
                        option { value: "", {t!("settlement-return-filter-any")} }
                        for status in ADMIN_SETTLEMENT_RETURN_STATUSES {
                            option { value: status, {label(status_key, status)} }
                        }
                    }
                }
                label {
                    {t!("settlement-return-filter-match")}
                    select {
                        value: current.match_state.clone(),
                        onchange: move |event| {
                            push_filter(&mut match_query, |next| {
                                next.match_state = closed_choice(
                                    &ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
                                    &event.value(),
                                );
                            });
                        },
                        option { value: "", {t!("settlement-return-filter-any")} }
                        for state in ADMIN_SETTLEMENT_RETURN_MATCH_STATES {
                            option { value: state, {label(match_key, state)} }
                        }
                    }
                }
                label {
                    {t!("settlement-return-filter-kind")}
                    select {
                        value: current.obligation_kind.clone(),
                        onchange: move |event| {
                            push_filter(&mut kind_query, |next| {
                                next.obligation_kind = closed_choice(
                                    &ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS,
                                    &event.value(),
                                );
                            });
                        },
                        option { value: "", {t!("settlement-return-filter-any")} }
                        for kind in ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS {
                            option { value: kind, {label(kind_key, kind)} }
                        }
                    }
                }
                label {
                    {t!("settlement-return-filter-world")}
                    select {
                        value: current.world,
                        onchange: move |event| {
                            push_filter(&mut world_query, |next| {
                                next.world = closed_choice(&["main", "isekai"], &event.value());
                            });
                        },
                        option { value: "", {t!("settlement-return-filter-any")} }
                        option { value: "main", {admin_world_label("main")} }
                        option { value: "isekai", {admin_world_label("isekai")} }
                    }
                }
            }
            {content}
        }
    }
}

/// 应用一项筛选变化并回到第一页，避免落在新结果中不存在的深页。
fn push_filter(query: &mut QueryState<ReturnsQuery>, change: impl FnOnce(&mut ReturnsQuery)) {
    let mut next = (query.state)();
    change(&mut next);
    next.page = 1;
    query.push(next);
}

/// 下拉框取值只接受闭集；其他值（含空串）回到“不限”。
fn closed_choice(allowed: &[&str], value: &str) -> String {
    if allowed.contains(&value) { value.to_owned() } else { String::new() }
}

#[component]
fn ReturnsTable(page: SettlementReturnPage) -> Element {
    if page.returns.is_empty() {
        return rsx! {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("settlement-return-empty"),
                detail: "",
            }
        };
    }
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-settlement-returns__table",
                aria_label: t!("page-settlement-returns-title"),
                thead {
                    tr {
                        th { scope: "col", {t!("settlement-return-column-received")} }
                        th { scope: "col", {t!("settlement-return-column-status")} }
                        th { scope: "col", {t!("settlement-return-column-location")} }
                        th { scope: "col", {t!("settlement-return-column-kind")} }
                        th { scope: "col", {t!("settlement-return-column-recipient")} }
                        th { scope: "col", {t!("settlement-return-column-assets")} }
                        th { scope: "col", {t!("settlement-return-column-match")} }
                        th { scope: "col", {t!("settlement-return-column-world")} }
                        th { scope: "col", {t!("settlement-return-column-case")} }
                    }
                }
                tbody {
                    for item in page.returns {
                        ReturnRow { key: "{item.return_id}", item }
                    }
                }
            }
        }
    }
}

#[component]
fn ReturnRow(item: SettlementReturnSummary) -> Element {
    let kind = item
        .obligation_kind
        .as_deref()
        .map_or_else(|| t!("value-empty"), |kind| label(kind_key, kind));
    let recipient = item.recipient.clone().unwrap_or_else(|| t!("value-empty"));
    let assets = assets_text(&item);
    let classification = format!(
        "{} · {}",
        label(match_key, &item.match_state),
        label(reason_key, &item.reason_code)
    );
    let short_id: String = item.return_id.chars().take(8).collect();
    rsx! {
        tr {
            td { "data-label": t!("settlement-return-column-received"), Time { value: item.received_at.clone() } }
            td { "data-label": t!("settlement-return-column-status"),
                span { class: status_badge(&item.status), {label(status_key, &item.status)} }
            }
            td { "data-label": t!("settlement-return-column-location"),
                if shows_asset_location(&item.status) {
                    {label(location_key, &item.asset_location)}
                } else {
                    {t!("settlement-return-location-resent-in-transit")}
                }
            }
            td { "data-label": t!("settlement-return-column-kind"), "{kind}" }
            td { "data-label": t!("settlement-return-column-recipient"),
                TruncatedText { value: recipient, portal: true }
            }
            td { "data-label": t!("settlement-return-column-assets"), "{assets}" }
            td { "data-label": t!("settlement-return-column-match"),
                TruncatedText { value: classification, size: "wide", portal: true }
            }
            td { "data-label": t!("settlement-return-column-world"), {admin_world_label(&item.world_id)} }
            td { "data-label": t!("settlement-return-column-case"),
                Link {
                    to: Route::SettlementReturnDetail { return_id: item.return_id.clone() },
                    title: item.return_id.clone(),
                    "{short_id}"
                }
            }
        }
    }
}

/// 退回资产摘要：Credits、COD 与附件件数，全部来自服务端冻结证据。
pub(super) fn assets_text(item: &SettlementReturnSummary) -> String {
    t!(
        "settlement-return-assets-value",
        credits: item.credits.to_string(),
        cod: item.cod.to_string(),
        attachments: item.attachment_count.to_string()
    )
}

/// 状态徽标：未结案按是否需要人工核对分级，已结案为中性。
fn status_badge(status: &str) -> &'static str {
    match status {
        "quarantined" | "take_unknown" => "admin-dashboard-badge admin-dashboard-badge--error",
        "awaiting_decision" | "recheck_pending" | "take_pending" | "resend_pending" => {
            "admin-dashboard-badge admin-dashboard-badge--warn"
        }
        _ => "admin-dashboard-badge admin-dashboard-badge--neutral",
    }
}

#[cfg(test)]
mod tests {
    use auction_contracts::ADMIN_SETTLEMENT_RETURN_STATUSES;

    use super::{closed_choice, status_badge};

    /// 筛选下拉框只接受闭集取值，其余回到不限。
    #[test]
    fn filter_choices_stay_in_the_closed_set() {
        assert_eq!(closed_choice(&ADMIN_SETTLEMENT_RETURN_STATUSES, "held"), "held");
        assert_eq!(closed_choice(&ADMIN_SETTLEMENT_RETURN_STATUSES, "open"), "");
        assert_eq!(closed_choice(&["main", "isekai"], ""), "");
    }

    /// 隔离与 Take 结果未知标为需要人工核对，处理中为警示，已结案为中性。
    #[test]
    fn status_badges_separate_review_pending_and_closed() {
        for status in ["quarantined", "take_unknown"] {
            assert!(status_badge(status).ends_with("--error"), "{status}");
        }
        for status in ["awaiting_decision", "recheck_pending", "take_pending", "resend_pending"] {
            assert!(status_badge(status).ends_with("--warn"), "{status}");
        }
        for status in ["held", "resent", "inbox_closed", "reviewed"] {
            assert!(status_badge(status).ends_with("--neutral"), "{status}");
        }
    }
}
