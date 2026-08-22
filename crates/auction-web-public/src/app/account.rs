//! Public 账户竞价与代理页面。
//!
//! 组件只展示服务端已经完成权限过滤的账户 projection。分页、代理状态和可执行能力不在
//! 浏览器端推导，后续命令仍需携带新的 `command_id` 并回到 Public API。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{MyBidsPage, OwnedBid, OwnedProxy, fetch_my_bids};

use super::Placeholder;

pub(super) fn page() -> Element {
    let account = use_resource(fetch_my_bids);
    match &*account.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-account-title"), detail: t!("session-loading") }
        },
        Some(Err(error)) if error == "HTTP 401" => rsx! {
            Placeholder { title: t!("page-account-title"), detail: t!("account-login-required") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-account-title"), detail: t!("data-error") }
        },
        Some(Ok(page)) => rsx! {
            AccountPage { page: page.clone() }
        },
    }
}

#[component]
fn AccountPage(page: MyBidsPage) -> Element {
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "account-bids-title",
            h1 { id: "account-bids-title", class: "public-auction-category__title",
                {t!("page-account-title")}
            }
            p { class: "public-auction-page__status",
                {t!("account-total", bids: page.bid_total, proxies: page.proxy_total)}
            }
            if page.bids.is_empty() {
                p { class: "public-auction-page__status", {t!("account-no-bids")} }
            } else {
                BidTable { bids: page.bids.clone() }
            }
            if page.proxies.is_empty() {
                p { class: "public-auction-page__status", {t!("account-no-proxies")} }
            } else {
                ProxyTable { proxies: page.proxies.clone() }
            }
        }
    }
}

#[component]
fn BidTable(bids: Vec<OwnedBid>) -> Element {
    rsx! {
        section { class: "public-auction-category",
            h2 { class: "public-auction-category__title", {t!("account-bids-title")} }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("account-lot")} }
                            th { {t!("account-item")} }
                            th { {t!("account-amount")} }
                            th { {t!("account-mechanism")} }
                            th { {t!("account-leading")} }
                            th { {t!("account-time")} }
                        }
                    }
                    tbody {
                        for bid in bids {
                            BidRow { bid }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn BidRow(bid: OwnedBid) -> Element {
    let leading = if bid.leading { t!("account-leading-yes") } else { t!("account-leading-no") };
    rsx! {
        tr {
            td { class: "public-auction-catalog__number", "{bid.lot_number}" }
            td { "{bid.equipment.name}" }
            td { class: "public-auction-catalog__amount public-auction-catalog__number", "{bid.amount}" }
            td { "{bid.mechanism}" }
            td { "{leading}" }
            td { time { datetime: bid.created_at.clone(), "{bid.created_at}" } }
        }
    }
}

#[component]
fn ProxyTable(proxies: Vec<OwnedProxy>) -> Element {
    rsx! {
        section { class: "public-auction-category",
            h2 { class: "public-auction-category__title", {t!("account-proxies-title")} }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("account-lot")} }
                            th { {t!("account-item")} }
                            th { {t!("account-mode")} }
                            th { {t!("account-maximum")} }
                            th { {t!("account-quota")} }
                            th { {t!("account-status")} }
                            th { {t!("account-capability")} }
                        }
                    }
                    tbody {
                        for proxy in proxies {
                            ProxyRow { proxy }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ProxyRow(proxy: OwnedProxy) -> Element {
    let terminal_reason = proxy.terminal_reason.unwrap_or_else(|| t!("value-empty"));
    let capability = if proxy.capabilities.can_lower {
        t!("account-capability-lower")
    } else {
        t!("account-capability-locked")
    };
    rsx! {
        tr {
            td { class: "public-auction-catalog__number", "{proxy.lot_number}" }
            td { "{proxy.equipment.name}" }
            td { "{proxy.mode}" }
            td { class: "public-auction-catalog__amount public-auction-catalog__number", "{proxy.maximum_amount}" }
            td { "{proxy.quota.used}/{proxy.quota.limit}" }
            td { "{proxy.status} ({terminal_reason})" }
            td { "{capability}" }
        }
    }
}
