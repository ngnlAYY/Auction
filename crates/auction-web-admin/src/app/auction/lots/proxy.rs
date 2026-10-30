//! Admin lot 的代理竞价历史弹窗与分页。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;

use auction_web_shared::{
    AsyncState, AsyncStateKind, Time, TruncatedText, format_credit_amount as format_amount,
};

use crate::api::{AdminProxyEvent, AdminProxyEventPage, fetch_admin_proxy_events};
use crate::app::surface::admin_api_error_message;
use crate::app::surface::pagination::{
    AdminPager as Pager, AdminPagination, use_admin_page, use_admin_pagination,
};

use super::helpers::{proxy_mode_label, proxy_status_label};

#[component]
pub(super) fn AdminProxyHistory(
    lot_id: String,
    lot_code: String,
    mut selected_lot: Signal<Option<(String, String)>>,
    mut reload_generation: Signal<u64>,
) -> Element {
    let page_number = use_signal(|| 1_i64);
    let pagination = use_admin_pagination("proxy-events", &["proxy_page_size"], Some(100), 100_000);
    let events = use_admin_page(
        pagination.clone(),
        move || {
            let _ = reload_generation();
            (lot_id.clone(), page_number())
        },
        |(lot_id, page), size| fetch_admin_proxy_events(lot_id, page, size),
    );

    let content = match &*events.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-lots-proxy-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-lots-retry"),
                on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
            }
        },
        Some(Ok(page)) => rsx! {
            AdminProxyHistoryTable { page: page.clone(), page_number, pagination }
        },
    };

    rsx! {
        // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；.admin-auction-proxy 内容网格
        // 样式移到面板内的包装 div 上（面板 class 已由 .ui-dialog__panel 统一）。
        Dialog {
            title: t!("admin-lots-proxy-title", lot: lot_code.clone()),
            close_label: t!("admin-lots-proxy-close"),
            on_close: move |_| selected_lot.set(None),
            div { class: "admin-auction-proxy",
                {content}
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        autofocus: true,
                        onclick: move |_| selected_lot.set(None),
                        {t!("admin-lots-proxy-close")}
                    }
                }
            }
        }
    }
}

#[component]
fn AdminProxyHistoryTable(
    page: AdminProxyEventPage,
    page_number: Signal<i64>,
    pagination: AdminPagination,
) -> Element {
    let current_page = page.pagination.page.unwrap_or(page_number()).max(1);
    let page_size = page.pagination.page_size.max(1);
    let total = page.pagination.total;
    let has_more =
        page.pagination.has_more.unwrap_or_else(|| page.pagination.next_cursor.is_some());

    rsx! {
        if page.proxy_events.is_empty() {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-lots-proxy-empty"),
                detail: "",
            }
        } else {
            div { class: "admin-auction-table-wrap",
                table { class: "admin-auction-table admin-auction-proxy__table",
                    caption { class: "admin-visually-hidden", {t!("admin-lots-proxy-pagination")} }
                    auction_web_shared::TableColumns { layout: "admin-proxy-history", widths: vec![22, 18, 10, 12, 10, 28] }
                    thead {
                        tr {
                            th { scope: "col", {t!("admin-lots-proxy-user")} }
                            th { scope: "col", {t!("admin-lots-proxy-maximum")} }
                            th { scope: "col", {t!("admin-lots-proxy-mode")} }
                            th { scope: "col", {t!("admin-lots-proxy-status")} }
                            th { scope: "col", {t!("admin-lots-proxy-usage")} }
                            th { scope: "col", {t!("admin-lots-proxy-created")} }
                        }
                    }
                    tbody {
                        for event in page.proxy_events.clone() {
                            AdminProxyHistoryRow { event }
                        }
                    }
                }
            }
        }
        Pager {
            pagination,
            local: true,
            page: current_page,
            page_size,
            total,
            has_more,
            label: t!("admin-lots-proxy-pagination"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}

#[component]
fn AdminProxyHistoryRow(event: AdminProxyEvent) -> Element {
    rsx! {
        tr {
            td { "data-label": t!("admin-lots-proxy-user"), strong { TruncatedText { value: event.username.clone() } } }
            td { "data-label": t!("admin-lots-proxy-maximum"),
                {t!("admin-lots-proxy-amount", amount: format_amount(event.maximum_amount))}
            }
            td { "data-label": t!("admin-lots-proxy-mode"), "{proxy_mode_label(&event.mode)}" }
            td { "data-label": t!("admin-lots-proxy-status"), "{proxy_status_label(&event.status)}" }
            td { "data-label": t!("admin-lots-proxy-usage"), title: event.authorization_id.clone(),
                {t!(
                    "admin-lots-proxy-identity",
                    authorization_sequence: event.authorization_sequence,
                    usage_number: event.usage_number
                )}
            }
            td { "data-label": t!("admin-lots-proxy-created"), Time { value: event.created_at.clone() } }
        }
    }
}
