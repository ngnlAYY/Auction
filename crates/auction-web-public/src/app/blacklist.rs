//! Public 黑名单页面。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{BlacklistEntry, BlacklistPage, fetch_blacklist};

use super::Placeholder;

pub(super) fn page() -> Element {
    let blacklist = use_resource(fetch_blacklist);
    match &*blacklist.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-blacklist-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-blacklist-title"), detail: t!("data-error") }
        },
        Some(Ok(page)) => rsx! {
            BlacklistTable { page: page.clone() }
        },
    }
}

#[component]
fn BlacklistTable(page: BlacklistPage) -> Element {
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "blacklist-title",
            h1 { id: "blacklist-title", class: "public-auction-category__title",
                {t!("page-blacklist-title")}
            }
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
        }
    }
}

#[component]
fn BlacklistRow(entry: BlacklistEntry) -> Element {
    rsx! {
        tr {
            td { "{entry.game_player_id}" }
            td { "{entry.reason}" }
            td {
                time { datetime: entry.blacklisted_at.clone(), "{entry.blacklisted_at}" }
            }
        }
    }
}
