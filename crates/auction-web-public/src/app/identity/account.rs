//! Public 账户竞价与代理页面。
//!
//! 页面只展示服务端已经完成权限过滤的账户 projection。普通竞价与代理授权分别使用旧版的
//! 独立页码，查询状态保留在 URL 中，登录后由服务端快照重新读取。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{MyBidsPage, OwnedBid, OwnedProxy, fetch_my_bids, lot_code_for};

use super::pagination::PublicPager;
use super::proxy::{
    proxy_mode_label, proxy_quota_label, proxy_status_label, proxy_transition_summary,
};
use super::{
    PublicSessionResource, Route, format_amount, format_auction_time,
    login_href_for_current_location, read_public_query, update_public_query,
};

const DEFAULT_ACCOUNT_PAGE_SIZE: i64 = 10;

pub(super) fn page() -> Element {
    let mut session = use_context::<PublicSessionResource>();
    let session_snapshot = session.read().clone();

    match session_snapshot.as_ref() {
        None => rsx! {
            section { class: "public-account-bids-page", aria_labelledby: "account-loading-title",
                h1 { id: "account-loading-title", {t!("page-account-title")} }
                p { class: "public-page-status", role: "status", {t!("data-loading")} }
            }
        },
        Some(Err(_)) => rsx! {
            section { class: "public-account-bids-page", aria_labelledby: "account-error-title",
                h1 { id: "account-error-title", {t!("page-account-title")} }
                div { class: "public-auction-empty", role: "alert",
                    p { class: "public-auction-empty__title", {t!("data-error")} }
                    button {
                        class: "public-auction-empty__retry",
                        r#type: "button",
                        onclick: move |_| session.restart(),
                        {t!("data-retry")}
                    }
                }
            }
        },
        Some(Ok(None)) | Some(Ok(Some(crate::api::PublicSession { authenticated: false, .. }))) => {
            rsx! {
                section { class: "public-account-bids-page", aria_labelledby: "account-login-title",
                    h1 { id: "account-login-title", {t!("page-account-title")} }
                    div { class: "public-account-login-prompt", role: "status",
                        p { {t!("account-login-required")} }
                        Link {
                            class: "public-account-login-cta",
                            to: login_href_for_current_location(),
                            {t!("account-login-cta")}
                        }
                    }
                }
            }
        }
        Some(Ok(Some(_))) => rsx! { AuthenticatedAccountData {} },
    }
}

/// 只有共享 session 已确认认证后才挂载受保护列表资源，对齐旧版请求序列。
#[component]
fn AuthenticatedAccountData() -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let mut account_reload_generation = use_signal(|| 0_u64);
    let mut bid_page = use_signal(|| read_public_query().bid_page);
    let mut proxy_page = use_signal(|| read_public_query().proxy_page);
    let account = use_resource(move || {
        let _ = refresh_generation();
        let _ = account_reload_generation();
        fetch_my_bids(bid_page(), proxy_page())
    });

    match &*account.read_unchecked() {
        None => rsx! {
            section { class: "public-account-bids-page", aria_labelledby: "account-loading-title",
                h1 { id: "account-loading-title", {t!("page-account-title")} }
                p { class: "public-page-status", role: "status", {t!("data-loading")} }
            }
        },
        Some(Err(_)) => rsx! {
            section { class: "public-account-bids-page", aria_labelledby: "account-error-title",
                h1 { id: "account-error-title", {t!("page-account-title")} }
                div { class: "public-auction-empty", role: "alert",
                    p { class: "public-auction-empty__title", {t!("data-error")} }
                    button {
                        class: "public-auction-empty__retry",
                        r#type: "button",
                        onclick: move |_| {
                            account_reload_generation
                                .set(account_reload_generation().saturating_add(1));
                        },
                        {t!("data-retry")}
                    }
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
        section { class: "public-account-bids-page", aria_labelledby: "account-bids-title",
            h1 { id: "account-bids-title", {t!("page-account-title")} }
            BidSection {
                bids: page.bids.clone(),
                page: bid_page,
                total: page.bid_total,
                page_size,
                on_page_change: on_bid_page_change,
            }
            ProxySection {
                proxies: page.proxies.clone(),
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
fn BidSection(
    bids: Vec<OwnedBid>,
    page: i64,
    total: i64,
    page_size: i64,
    on_page_change: EventHandler<i64>,
) -> Element {
    rsx! {
        section { class: "public-account-section",
            h2 { {t!("account-bids-title")} }
            if bids.is_empty() {
                p { class: "public-account-empty", {t!("account-no-bids")} }
            } else {
                div { class: "public-account-table-wrap",
                    table { class: "public-account-table",
                        thead {
                            tr {
                                th { {t!("account-lot")} }
                                th { class: "numeric-column public-account-number", {t!("account-amount")} }
                                th { {t!("account-bid-status")} }
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
            AccountPager {
                label: t!("account-bids-title"),
                page,
                total,
                page_size,
                on_page_change,
            }
        }
    }
}

#[component]
fn BidRow(bid: OwnedBid) -> Element {
    let leading = if bid.leading { t!("account-bid-leading") } else { t!("account-bid-outbid") };
    let row_class = if bid.leading { "public-account-leading" } else { "" };
    rsx! {
        tr { class: row_class,
            td { class: "public-account-code", "data-label": t!("account-lot"),
                Link {
                    to: Route::LotDetail { lot_code: bid.lot_id.clone() },
                    {lot_code_for(&bid.equipment.category, bid.lot_number)}
                }
            }
            td { class: "public-account-amount public-account-number numeric-column", "data-label": t!("account-amount"), {format_amount(bid.amount)} }
            td { "data-label": t!("account-bid-status"),
                span { class: if bid.leading { "public-account-status public-account-status--live" } else { "public-account-status" }, {leading} }
            }
            td { "data-label": t!("account-time"), time { datetime: bid.created_at.clone(), "{format_auction_time(&bid.created_at)}" } }
        }
    }
}

#[component]
fn ProxySection(
    proxies: Vec<OwnedProxy>,
    page: i64,
    total: i64,
    page_size: i64,
    on_page_change: EventHandler<i64>,
) -> Element {
    rsx! {
        section { class: "public-account-section",
            h2 { {t!("account-proxies-title")} }
            p { class: "public-account-hint", {t!("account-proxies-note")} }
            if proxies.is_empty() {
                p { class: "public-account-empty", {t!("account-no-proxies")} }
            } else {
                div { class: "public-account-table-wrap",
                    table { class: "public-account-table",
                        thead {
                            tr {
                                th { {t!("account-lot")} }
                                th { class: "numeric-column public-account-number", {t!("account-secret-maximum")} }
                                th { {t!("account-transition-history")} }
                                th { {t!("account-mode")} }
                                th { {t!("account-proxy-uses")} }
                                th { {t!("account-status")} }
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
            AccountPager {
                label: t!("account-proxies-title"),
                page,
                total,
                page_size,
                on_page_change,
            }
        }
    }
}

#[component]
fn ProxyRow(proxy: OwnedProxy) -> Element {
    let transitions = proxy_transition_summary(&proxy);
    let quota = proxy_quota_label(&proxy);
    rsx! {
        tr {
            td { class: "public-account-code", "data-label": t!("account-lot"),
                Link {
                    to: Route::LotDetail { lot_code: proxy.lot_id.clone() },
                    {lot_code_for(&proxy.equipment.category, proxy.lot_number)}
                }
            }
            td { class: "public-account-amount public-account-number numeric-column", "data-label": t!("account-secret-maximum"), {format_amount(proxy.maximum_amount)} }
            td { "data-label": t!("account-transition-history"), {transitions} }
            td { "data-label": t!("account-mode"), {proxy_mode_label(&proxy.mode)} }
            td { "data-label": t!("account-proxy-uses"), {quota} }
            td { "data-label": t!("account-status"), {proxy_status_label(&proxy.status)} }
        }
    }
}
