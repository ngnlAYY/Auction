//! Public 账户当前标记装备、竞价与代理页面。
//! 页面只展示服务端已经完成权限过滤的账户 projection。普通竞价与代理授权分别使用旧版的
//! 独立页码，查询状态保留在 URL 中，登录后由服务端快照重新读取。

use auction_web_shared::{LanguagePicker, ThemePicker, legacy_t as t};
use dioxus::prelude::*;

use crate::api::{MyBidsPage, OwnedBid, OwnedProxy, fetch_my_bids, lot_code_for};
use crate::surface::{PublicCommandOwner, use_bid_refresh};

use super::proxy::{
    proxy_mode_label, proxy_quota_label, proxy_status_label, proxy_transition_summary,
};
use super::session_preference::SessionPreference;
use super::watch_preference::OutbidNotifications;
use super::{
    PublicPager, PublicSessionResource, QueryWriteMode, Time, format_amount,
    login_href_for_current_location, lot_detail_auction_href, update_public_query_state,
    use_public_query_state,
};

const DEFAULT_ACCOUNT_PAGE_SIZE: i64 = 10;

mod watches;

pub(super) fn page() -> Element {
    let mut session = use_context::<PublicSessionResource>();
    let command_owner = use_context::<PublicCommandOwner>().0;
    let session_snapshot = session.read();
    let account = match session_snapshot.as_ref() {
        None => rsx! {
            p { class: "public-page-status", role: "status", {t!("data-loading")} }
        },
        Some(Err(_)) => rsx! {
            div { class: "public-auction-empty", role: "alert",
                p { class: "public-auction-empty__title", {t!("data-error")} }
                button {
                    class: "public-auction-empty__retry",
                    r#type: "button",
                    onclick: move |_| session.restart(),
                    {t!("data-retry")}
                }
            }
        },
        Some(Ok(None)) | Some(Ok(Some(crate::api::PublicSession { authenticated: false, .. }))) => {
            rsx! {
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
        Some(Ok(Some(identity))) => rsx! {
            for user_id in [identity.user_id.clone()] {
                AccountReadScope { key: "account-{user_id}", user_id }
            }
        },
    };

    rsx! {
        section { class: "public-account-bids-page", aria_labelledby: "account-title",
            h1 { id: "account-title", {t!("page-account-title")} }
            if command_owner().is_some() {
                super::presentation::mobile::AccountNavigation {}
            }
            // 会话暂时读取失败只禁用新写；与竞价宿主一样，明确换号或登出才销毁未知命令。
            for user_id in command_owner() {
                AccountPreferenceScope { key: "preferences-{user_id}", user_id }
            }
            {account}
        }
    }
}

/// 同一稳定账号的两个偏好命令共用 keyed 宿主；布局容器不依赖宽度或会话读取分支。
#[component]
fn AccountPreferenceScope(user_id: String) -> Element {
    rsx! {
        div { class: "public-account-preferences",
            section {
                id: "display-preferences",
                class: "public-account-display-preferences",
                aria_labelledby: "display-preferences-title",
                h2 { id: "display-preferences-title", {t!("account-display-title")} }
                p { class: "public-account-display-preferences__hint",
                    {t!("account-display-hint")}
                }
                div { class: "public-account-display-preferences__controls",
                    LanguagePicker {
                        label: t!("language-label"),
                        catalog: &super::router::PUBLIC_LOCALE_CATALOG,
                    }
                    ThemePicker {
                        label: t!("theme-label"),
                        dark_label: t!("theme-dark"),
                        light_label: t!("theme-light"),
                    }
                }
            }
            OutbidNotifications { user_id: user_id.clone() }
            SessionPreference { user_id }
        }
    }
}

/// 整组只读列表以账号隔离；换号时同时重建标记及竞价读取，避免复用另一账号的快照。
#[component]
fn AccountReadScope(user_id: String) -> Element {
    rsx! {
        watches::CurrentWatches { user_id: user_id.clone() }
        AuthenticatedAccountData { user_id }
    }
}

/// 只有共享 session 已确认认证后才挂载受保护列表资源，对齐旧版请求序列。
#[component]
fn AuthenticatedAccountData(user_id: String) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let mut account_reload_generation = use_signal(|| 0_u64);
    // 竞价与代理页码与地址栏双向同步：初值取自 URL，浏览器前进/后退由 hook 恢复。
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    let bid_page = use_memo(move || query_state_signal.read().bid_page);
    let proxy_page = use_memo(move || query_state_signal.read().proxy_page);
    let account = use_resource(move || {
        let _ = refresh_generation();
        let _ = account_reload_generation();
        let requested = (bid_page(), proxy_page());
        let user_id = user_id.clone();
        async move {
            let result = fetch_my_bids(&user_id, requested.0, requested.1).await;
            (requested, result)
        }
    });
    use_bid_refresh(account, true);

    let loaded = account.read_unchecked();
    let current = loaded
        .as_ref()
        .filter(|(requested, _)| *requested == (bid_page(), proxy_page()))
        .map(|(_, result)| result);
    match current {
        None => rsx! {
            div {
                p { class: "public-page-status", role: "status", {t!("data-loading")} }
            }
        },
        Some(Err(_)) => rsx! {
            div {
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
        Some(Ok(page)) if page.bid_page != bid_page() || page.proxy_page != proxy_page() => rsx! {
            p { class: "public-page-status", role: "status", {t!("data-loading")} }
        },
        Some(Ok(page)) => rsx! {
            AccountPage {
                page: page.clone(),
                bid_page: bid_page(),
                proxy_page: proxy_page(),
                on_bid_page_change: move |next_page| {
                    update_public_query_state(
                        query_state_signal,
                        |query| query.bid_page = next_page,
                        QueryWriteMode::Push,
                    );
                },
                on_bid_page_clamp: move |next_page| {
                    update_public_query_state(
                        query_state_signal,
                        |query| query.bid_page = next_page,
                        QueryWriteMode::Replace,
                    );
                },
                on_proxy_page_change: move |next_page| {
                    update_public_query_state(
                        query_state_signal,
                        |query| query.proxy_page = next_page,
                        QueryWriteMode::Push,
                    );
                },
                on_proxy_page_clamp: move |next_page| {
                    update_public_query_state(
                        query_state_signal,
                        |query| query.proxy_page = next_page,
                        QueryWriteMode::Replace,
                    );
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
    on_bid_page_clamp: EventHandler<i64>,
    on_proxy_page_change: EventHandler<i64>,
    on_proxy_page_clamp: EventHandler<i64>,
) -> Element {
    let page_size = if page.page_size > 0 { page.page_size } else { DEFAULT_ACCOUNT_PAGE_SIZE };
    rsx! {
        div { class: "public-account-activity",
            BidSection {
                bids: page.bids,
                page: bid_page,
                total: page.bid_total,
                page_size,
                on_page_change: on_bid_page_change,
                on_page_clamp: on_bid_page_clamp,
            }
            ProxySection {
                proxies: page.proxies,
                page: proxy_page,
                total: page.proxy_total,
                page_size,
                on_page_change: on_proxy_page_change,
                on_page_clamp: on_proxy_page_clamp,
            }
        }
    }
}

#[component]
fn BidSection(
    bids: Vec<OwnedBid>,
    page: i64,
    total: i64,
    page_size: i64,
    on_page_change: EventHandler<i64>,
    on_page_clamp: EventHandler<i64>,
) -> Element {
    rsx! {
        section { class: "public-account-section", aria_labelledby: "account-bids-title",
            h2 { id: "account-bids-title", {t!("account-bids-title")} }
            if bids.is_empty() {
                p { class: "public-account-empty", {t!("account-no-bids")} }
            } else {
                div { class: "public-account-table-wrap",
                    table { class: "public-account-table",
                        caption { class: "public-visually-hidden", {t!("account-bids-title")} }
                        auction_web_shared::TableColumns { layout: "public-account-bids", widths: vec![18, 28, 54] }
                        thead {
                            tr {
                                th { scope: "col", {t!("account-lot")} }
                                th { scope: "col", class: "numeric-column public-account-number", {t!("account-amount")} }
                                th { scope: "col", {t!("account-time")} }
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
            PublicPager {
                label: t!("account-bids-title"),
                page,
                total: Some(total),
                page_size,
                hide_when_single: true,
                show_size_picker: false,
                max_offset: None,
                jump_label: Some(t!("catalog-page-input")),
                on_page_change,
                on_page_clamp,
            }
        }
    }
}

#[component]
fn BidRow(bid: OwnedBid) -> Element {
    let row_class = if bid.leading { "public-account-leading" } else { "" };
    let bid_status = if bid.leading { t!("account-bid-leading") } else { t!("account-bid-outbid") };
    let bid_status_id = format!("public-account-bid-status-{}", bid.id);
    rsx! {
        tr {
            class: row_class,
            aria_describedby: bid_status_id.clone(),
            td { class: "public-account-code", "data-label": t!("account-lot"),
                Link {
                    to: lot_detail_auction_href(&bid.lot_id, &bid.auction_id),
                    {lot_code_for(&bid.equipment.category, bid.lot_number)}
                }
                // 可见领先状态由装备行高亮表达；隐藏文案保留非视觉用户可读取的同等语义。
                span {
                    id: bid_status_id.clone(),
                    class: "public-visually-hidden",
                    "{bid_status}"
                }
            }
            td { class: "public-account-amount public-account-number numeric-column", "data-label": t!("account-amount"), {format_amount(bid.amount)} }
            td { "data-label": t!("account-time"), Time { value: bid.created_at.clone() } }
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
    on_page_clamp: EventHandler<i64>,
) -> Element {
    rsx! {
        section { class: "public-account-section", aria_labelledby: "account-proxies-title",
            h2 { id: "account-proxies-title", {t!("account-proxies-title")} }
            if proxies.is_empty() {
                p { class: "public-account-empty", {t!("account-no-proxies")} }
            } else {
                div { class: "public-account-table-wrap",
                    table { class: "public-account-table",
                        caption { class: "public-visually-hidden", {t!("account-proxies-title")} }
                        auction_web_shared::TableColumns { layout: "public-account-proxies", widths: vec![10, 18, 38, 10, 10, 14] }
                        thead {
                            tr {
                                th { scope: "col", {t!("account-lot")} }
                                th { scope: "col", class: "numeric-column public-account-number", {t!("account-secret-maximum")} }
                                th { scope: "col", {t!("account-transition-history")} }
                                th { scope: "col", {t!("account-mode")} }
                                th { scope: "col", {t!("account-proxy-uses")} }
                                th { scope: "col", {t!("account-status")} }
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
            PublicPager {
                label: t!("account-proxies-title"),
                page,
                total: Some(total),
                page_size,
                hide_when_single: true,
                show_size_picker: false,
                max_offset: None,
                jump_label: Some(t!("catalog-page-input")),
                on_page_change,
                on_page_clamp,
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
                    to: lot_detail_auction_href(&proxy.lot_id, &proxy.auction_id),
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
