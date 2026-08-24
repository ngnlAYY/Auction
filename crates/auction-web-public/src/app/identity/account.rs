//! Public 账户竞价与代理页面。
//!
//! 页面只展示服务端已经完成权限过滤的账户 projection。普通竞价与代理授权分别使用旧版的
//! 独立页码，查询状态保留在 URL 中，登录后由服务端快照重新读取。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{MyBidsPage, OwnedBid, OwnedProxy, fetch_my_bids, lot_code_for};

use super::pagination::PublicPager;
use super::proxy::{
    proxy_capability_label, proxy_mode_label, proxy_quota_label, proxy_status_label,
    proxy_transition_summary,
};
use super::{
    Placeholder, Route, format_amount, format_auction_time, read_public_query, update_public_query,
};

const DEFAULT_ACCOUNT_PAGE_SIZE: i64 = 20;

pub(super) fn page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let mut bid_page = use_signal(|| read_public_query().bid_page);
    let mut proxy_page = use_signal(|| read_public_query().proxy_page);
    let account = use_resource(move || {
        let _ = refresh_generation();
        fetch_my_bids(bid_page(), proxy_page())
    });

    match &*account.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-account-title"), detail: t!("data-loading") }
        },
        Some(Err(error)) if error == "HTTP 401" => rsx! {
            section {
                class: "public-auction-empty public-account-login-prompt",
                role: "status",
                aria_labelledby: "account-login-title",
                h1 { id: "account-login-title", {t!("page-account-title")} }
                p { {t!("account-login-required")} }
                Link { class: "public-account-login-cta", to: Route::Login {}, {t!("account-login-cta")} }
            }
        },
        Some(Err(_)) => rsx! {
            section {
                class: "public-auction-empty",
                role: "alert",
                aria_labelledby: "account-error-title",
                h1 { id: "account-error-title", {t!("page-account-title")} }
                p { {t!("data-error")} }
                button {
                    r#type: "button",
                    onclick: move |_| refresh_generation.set(refresh_generation().saturating_add(1)),
                    {t!("data-retry")}
                }
            }
        },
        Some(Ok(page)) => rsx! {
            AccountPage {
                page: page.clone(),
                bid_page: bid_page(),
                proxy_page: proxy_page(),
                on_bid_page_change: move |next_page| {
                    bid_page.set(next_page);
                    update_public_query(|query| query.bid_page = next_page, false);
                },
                on_proxy_page_change: move |next_page| {
                    proxy_page.set(next_page);
                    update_public_query(|query| query.proxy_page = next_page, false);
                },
            }
        },
    }
}

#[component]
fn AccountPage(
    page: MyBidsPage,
    bid_page: i64,
    proxy_page: i64,
    on_bid_page_change: EventHandler<i64>,
    on_proxy_page_change: EventHandler<i64>,
) -> Element {
    let page_size = if page.page_size > 0 { page.page_size } else { DEFAULT_ACCOUNT_PAGE_SIZE };
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "account-bids-title",
            h1 { id: "account-bids-title", class: "public-auction-category__title",
                {t!("page-account-title")}
            }
            p { class: "public-auction-page__status", {t!("page-account-note")} }
            p { class: "public-auction-page__status",
                {t!("account-total", bids: page.bid_total, proxies: page.proxy_total)}
            }
            if page.bids.is_empty() {
                p { class: "public-auction-page__status", {t!("account-no-bids")} }
            } else {
                BidTable { bids: page.bids.clone() }
            }
            AccountPager {
                label: t!("account-bids-title"),
                page: bid_page,
                total: page.bid_total,
                page_size,
                on_page_change: on_bid_page_change,
            }
            if page.proxies.is_empty() {
                p { class: "public-auction-page__status", {t!("account-no-proxies")} }
            } else {
                ProxyTable { proxies: page.proxies.clone() }
            }
            AccountPager {
                label: t!("account-proxies-title"),
                page: proxy_page,
                total: page.proxy_total,
                page_size,
                on_page_change: on_proxy_page_change,
            }
        }
    }
}

#[component]
fn AccountPager(
    label: String,
    page: i64,
    total: i64,
    page_size: i64,
    on_page_change: EventHandler<i64>,
) -> Element {
    rsx! {
        PublicPager { page, page_size, total: Some(total), has_more: false, label, on_page_change }
    }
}

#[component]
fn BidTable(bids: Vec<OwnedBid>) -> Element {
    rsx! {
        section { class: "public-account-section",
            h2 { class: "public-auction-category__title", {t!("account-bids-title")} }
            div { class: "public-auction-catalog public-account-table-wrap",
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
    let leading = if bid.leading { t!("account-bid-leading") } else { t!("account-bid-outbid") };
    let row_class = if bid.leading { "public-auction-catalog__leading" } else { "" };
    rsx! {
        tr { class: row_class,
            td { class: "public-auction-catalog__code", "data-label": t!("account-lot"),
                Link {
                    to: Route::LotDetail { lot_code: bid.lot_id.clone() },
                    {lot_code_for(&bid.equipment.category, bid.lot_number)}
                }
            }
            td { "data-label": t!("account-item"), "{bid.equipment.name}" }
            td { class: "public-auction-catalog__amount public-auction-catalog__number", "data-label": t!("account-amount"), {format_amount(bid.amount)} }
            td { "data-label": t!("account-mechanism"), {super::bid_mechanism_label(&bid.mechanism)} }
            td { "data-label": t!("account-leading"),
                span { class: if bid.leading { "public-auction-status public-auction-status--live" } else { "public-auction-status" }, {leading} }
            }
            td { "data-label": t!("account-time"), time { datetime: bid.created_at.clone(), "{format_auction_time(&bid.created_at)}" } }
        }
    }
}

#[component]
fn ProxyTable(proxies: Vec<OwnedProxy>) -> Element {
    rsx! {
        section { class: "public-account-section",
            h2 { class: "public-auction-category__title", {t!("account-proxies-title")} }
            p { class: "public-auction-page__status", {t!("account-proxies-note")} }
            div { class: "public-auction-catalog public-account-table-wrap",
                table {
                    thead {
                        tr {
                            th { {t!("account-lot")} }
                            th { {t!("account-item")} }
                            th { {t!("account-mode")} }
                            th { {t!("account-maximum")} }
                            th { {t!("account-transition-history")} }
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
    let transitions = proxy_transition_summary(&proxy);
    let quota = proxy_quota_label(&proxy);
    let capability = proxy_capability_label(&proxy);
    rsx! {
        tr {
            td { class: "public-auction-catalog__code", "data-label": t!("account-lot"),
                Link {
                    to: Route::LotDetail { lot_code: proxy.lot_id.clone() },
                    {lot_code_for(&proxy.equipment.category, proxy.lot_number)}
                }
            }
            td { "data-label": t!("account-item"), "{proxy.equipment.name}" }
            td { "data-label": t!("account-mode"), {proxy_mode_label(&proxy.mode)} }
            td { class: "public-auction-catalog__amount public-auction-catalog__number", "data-label": t!("account-maximum"), {format_amount(proxy.maximum_amount)} }
            td { "data-label": t!("account-transition-history"), {transitions} }
            td { "data-label": t!("account-quota"), {quota} }
            td { "data-label": t!("account-status"), {proxy_status_label(&proxy.status)} }
            td { "data-label": t!("account-capability"), {capability} }
        }
    }
}
