//! Public 黑名单页面。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::TruncatedText;

use crate::api::{BlacklistEntry, BlacklistPage, fetch_blacklist};

use super::{
    AsyncState, AsyncStateKind, Pager, QueryWriteMode, Time, update_public_query_state,
    use_public_query_state,
};

pub(super) fn page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 页码与地址栏双向同步：初值取自 URL，浏览器前进/后退由 hook 恢复。
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    let page_number = use_memo(move || query_state_signal().page.max(1));
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
            BlacklistTable {
                page: page.clone(),
                on_page_change: move |next_page| {
                    update_public_query_state(
                        query_state_signal,
                        |query| query.page = next_page,
                        QueryWriteMode::Push,
                    );
                },
            }
        },
    }
}

#[component]
fn BlacklistTable(page: BlacklistPage, on_page_change: EventHandler<i64>) -> Element {
    rsx! {
        section { class: "public-blacklist-page", aria_labelledby: "blacklist-title",
            h1 { id: "blacklist-title", {t!("page-blacklist-title")} }
            if page.users.is_empty() {
                p { class: "public-blacklist-empty", {t!("blacklist-empty")} }
            } else {
                div { class: "public-blacklist-table-wrap",
                    table { class: "public-blacklist-table",
                        // 列序：玩家、理由、时间；按字段预期内容分配比例。
                        auction_web_shared::TableColumns { layout: "public-blacklist", widths: vec![22, 54, 24] }
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
                on_page_change,
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
