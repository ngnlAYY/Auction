//! Public 黑名单页面。

use dioxus::prelude::*;
use dioxus_i18n::t;

use auction_web_shared::TruncatedText;

use crate::api::{BlacklistEntry, BlacklistPage, fetch_blacklist};

use super::pagination::PublicPager;
use super::{
    Placeholder, RetryPlaceholder, format_auction_time, read_public_query, update_public_query,
};

pub(super) fn page() -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let page_number = use_signal(|| read_public_query().page.max(1));
    let blacklist = use_resource(move || {
        let _ = refresh_generation();
        fetch_blacklist(page_number())
    });
    match &*blacklist.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-blacklist-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-blacklist-title"),
                detail: t!("data-error"),
                refresh_generation,
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
        section { class: "public-auction-category", aria_labelledby: "blacklist-title",
            h1 { id: "blacklist-title", class: "public-auction-category__title",
                {t!("page-blacklist-title")}
            }
            p { class: "public-auction-page__status", {t!("page-blacklist-note")} }
            p { class: "public-auction-page__status", {t!("blacklist-total", total: page.total)} }
            if page.users.is_empty() {
                p { class: "public-auction-page__status", {t!("blacklist-empty")} }
            } else {
                div { class: "public-auction-catalog",
                    table {
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
            PublicPager {
                page: page.page,
                page_size: page.page_size,
                total: Some(page.total),
                has_more: false,
                label: t!("page-blacklist-title"),
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
                TruncatedText { value: entry.game_player_id }
            }
            td { "data-label": t!("blacklist-reason"),
                TruncatedText { value: entry.reason, size: "wide" }
            }
            td {
                "data-label": t!("blacklist-time"),
                time {
                    datetime: entry.blacklisted_at.clone(),
                    "{format_auction_time(&entry.blacklisted_at)}"
                }
            }
        }
    }
}
