//! Admin lot 的代理竞价历史弹窗与分页。

use auction_web_shared::legacy_t as t;
use dioxus::{html::Key, prelude::*};

use auction_web_shared::{TruncatedText, format_credit_amount as format_amount};

use crate::api::{AdminProxyEvent, AdminProxyEventPage, fetch_admin_proxy_events};
use crate::app::surface::admin_api_error_message;

use super::super::format_admin_time;
use super::super::pagination::AdminPager;
use super::helpers::{proxy_mode_label, proxy_status_label};

#[component]
pub(super) fn AdminProxyHistory(
    lot_id: String,
    lot_number: i32,
    mut selected_lot: Signal<Option<(String, i32)>>,
    mut reload_generation: Signal<u64>,
) -> Element {
    let page_number = use_signal(|| 1_i64);
    let events = use_resource({
        let lot_id = lot_id.clone();
        move || {
            let _ = reload_generation();
            fetch_admin_proxy_events(lot_id.clone(), page_number())
        }
    });

    let content = match &*events.read_unchecked() {
        None => rsx! {
            p { role: "status", {t!("admin-lots-proxy-loading")} }
        },
        Some(Err(error)) => rsx! {
            div {
                p { role: "alert", {admin_api_error_message(error)} }
                button {
                    r#type: "button",
                    onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                    {t!("admin-lots-retry")}
                }
            }
        },
        Some(Ok(page)) => rsx! {
            AdminProxyHistoryTable { page: page.clone(), page_number }
        },
    };

    rsx! {
        div { class: "admin-shell__dialog-backdrop", role: "presentation",
            section {
                class: "admin-shell__dialog admin-auction-proxy",
                role: "dialog",
                aria_modal: "true",
                aria_label: t!("admin-lots-proxy-title", lot: lot_number),
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        event.stop_propagation();
                        selected_lot.set(None);
                    }
                },
                h2 { {t!("admin-lots-proxy-title", lot: lot_number)} }
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
fn AdminProxyHistoryTable(page: AdminProxyEventPage, page_number: Signal<i64>) -> Element {
    let current_page = page.pagination.page.unwrap_or(page_number()).max(1);
    let page_size = page.pagination.page_size.max(1);
    let total = proxy_event_total(&page, current_page);

    rsx! {
        if page.proxy_events.is_empty() {
            p { {t!("admin-lots-proxy-empty")} }
        } else {
            div { class: "admin-auction-table-wrap",
                table { class: "admin-auction-table",
                    tbody {
                        for event in page.proxy_events.clone() {
                            AdminProxyHistoryRow { event }
                        }
                    }
                }
            }
        }
        AdminPager {
            page: current_page,
            page_size,
            total,
            label: t!("admin-lots-proxy-pagination"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}

fn proxy_event_total(page: &AdminProxyEventPage, current_page: i64) -> i64 {
    let page_size = page.pagination.page_size.max(1);
    if let Some(total) = page.pagination.total {
        return total;
    }

    let has_more =
        page.pagination.has_more.unwrap_or_else(|| page.pagination.next_cursor.is_some());
    let loaded = current_page.saturating_mul(page_size);
    if has_more {
        loaded.saturating_add(1)
    } else {
        current_page
            .saturating_sub(1)
            .saturating_mul(page_size)
            .saturating_add(page.proxy_events.len().try_into().unwrap_or(i64::MAX))
    }
}

#[component]
fn AdminProxyHistoryRow(event: AdminProxyEvent) -> Element {
    rsx! {
        tr {
            td { strong { TruncatedText { value: event.username.clone() } } }
            td {
                {t!("admin-lots-proxy-amount", amount: format_amount(event.maximum_amount))}
            }
            td { "{proxy_mode_label(&event.mode)}" }
            td { "{proxy_status_label(&event.status)}" }
            td { title: event.authorization_id.clone(),
                {t!(
                    "admin-lots-proxy-identity",
                    authorization_sequence: event.authorization_sequence,
                    usage_number: event.usage_number
                )}
            }
            td { time { datetime: event.created_at.clone(), "{format_admin_time(&event.created_at)}" } }
        }
    }
}
