//! Public 黑名单页面。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::TruncatedText;

use crate::api::{BlacklistEntry, BlacklistPage, fetch_blacklist};

use super::{AsyncState, AsyncStateKind, Pager, Time, read_public_query, update_public_query};

pub(super) fn page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let page_number = use_signal(|| read_public_query().page.max(1));
    let blacklist = use_resource(move || {
        let _ = refresh_generation();
        fetch_blacklist(page_number())
    });
    match &*blacklist.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("page-blacklist-title"),
                detail: t!("data-loading"),
            }
        },
        Some(Err(_)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("page-blacklist-title"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        },
        Some(Ok(page)) => rsx! {
            BlacklistTable { page: page.clone(), page_number }
        },
    }
}

#[component]
fn BlacklistTable(page: BlacklistPage, mut page_number: Signal<i64>) -> Element {
    rsx! {
        section { class: "public-blacklist-page", aria_labelledby: "blacklist-title",
            h1 { id: "blacklist-title", {t!("page-blacklist-title")} }
            p { {t!("page-blacklist-note")} }
            if page.users.is_empty() {
                p { class: "public-blacklist-empty", {t!("blacklist-empty")} }
            } else {
                div { class: "public-blacklist-table-wrap",
                    table { class: "public-blacklist-table",
                        thead {
                            tr {
                                th { {t!("blacklist-player")} }
                                th { {t!("blacklist-reason")} }
                                th { {t!("blacklist-time")} }
                            }
                        }
                        tbody {
                            for entry in page.users {
                                BlacklistRow { entry }
                            }
                        }
                    }
                }
            }
            Pager {
                page: page.page,
                page_size: page.page_size,
                total: Some(page.total),
                has_more: false,
                label: t!("page-blacklist-title"),
                previous_label: t!("catalog-previous"),
                status: move |(page, pages)| t!("catalog-page", page: page, pages: pages),
                next_label: t!("catalog-next"),
                jump_label: t!("catalog-page-input"),
                jump_button_label: t!("catalog-page-go"),
                on_page_change: move |next_page| {
                    page_number.set(next_page);
                    update_public_query(|query| query.page = next_page, false);
                },
            }
        }
    }
}

#[component]
fn BlacklistRow(entry: BlacklistEntry) -> Element {
    rsx! {
        tr {
            td { "data-label": t!("blacklist-player"),
                TruncatedText { value: entry.username, portal: true }
            }
            td { "data-label": t!("blacklist-reason"),
                TruncatedText { value: entry.reason, size: "wide", portal: true }
            }
            td {
                "data-label": t!("blacklist-time"),
                Time { value: entry.blacklisted_at.clone() }
            }
        }
    }
}
