//! Public 账户当前标记装备、竞价与代理页面。
//! 页面只展示服务端已经完成权限过滤的账户 projection。普通竞价与代理授权分别使用旧版的
//! 独立页码，查询状态保留在 URL 中，登录后由服务端快照重新读取。

use auction_web_shared::{QueryWriteMode, Time, TruncatedText, format_credit_amount, t};
use dioxus::prelude::*;

use super::proxy::{
    proxy_mode_label, proxy_quota_label, proxy_status_label, proxy_transition_summary,
};
use super::session_preference::SessionPreference;
use super::watch_preference::OutbidNotifications;
use super::{
    PageKey, PublicPageClampError, PublicPager, PublicSessionResource,
    login_href_for_current_location, public_page_within_offset, use_content_loading,
    use_page_query, use_public_page_bounds,
};
use crate::api::{MyBidsPage, OwnedBid, OwnedProxy, fetch_my_bids, lot_code_for};
#[cfg(test)]
use crate::surface::PageReadFixture;
use crate::surface::{
    LotDetailLookupButton, LotDetailLookupHost, PublicCommandOwner, use_bid_refresh,
    use_lot_detail_lookup,
};

// 与 /me/bids 的固定请求页长一致。
const DEFAULT_ACCOUNT_PAGE_SIZE: i64 = 10;
const ACCOUNT_PATH: &str = "/me";

pub(super) mod preference_reload;
mod watches;

pub(super) fn page() -> Element {
    let mut session = use_context::<PublicSessionResource>();
    let command_owner = use_context::<PublicCommandOwner>().0;
    let session_snapshot = session.read();
    use_content_loading(session_snapshot.is_none());
    let account = match session_snapshot.as_ref() {
        None => rsx! {},
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
        section { class: "public-account-bids-page", aria_label: t!("page-account-title"),
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
            OutbidNotifications { user_id: user_id.clone() }
            SessionPreference { user_id }
        }
    }
}

/// 整组只读列表以账号隔离；换号时同时重建标记及竞价读取，避免复用另一账号的快照。
#[component]
fn AccountReadScope(user_id: String) -> Element {
    // 标记、出价与代理三张列表共用一个只读详情抽屉；宿主在各自的读取分支之外，
    // 换页或重读进入加载分支时已打开的抽屉不被卸载。
    let lookup = use_lot_detail_lookup();
    rsx! {
        watches::CurrentWatches { user_id: user_id.clone() }
        AuthenticatedAccountData { user_id }
        LotDetailLookupHost { lookup }
    }
}

/// 只有共享 session 已确认认证后才挂载受保护列表资源，对齐旧版请求序列。
#[component]
fn AuthenticatedAccountData(user_id: String) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let mut account_reload_generation = use_signal(|| 0_u64);
    // 竞价与代理页码与地址栏双向同步。
    let query_state_signal = use_page_query();
    let bid_page = use_memo(move || query_state_signal.read().bid_page);
    let proxy_page = use_memo(move || query_state_signal.read().proxy_page);
    let bounds = use_public_page_bounds(query_state_signal, ACCOUNT_PATH, |_| {
        [(PageKey::Bid, DEFAULT_ACCOUNT_PAGE_SIZE), (PageKey::Proxy, DEFAULT_ACCOUNT_PAGE_SIZE)]
    });
    let in_range = use_memo(move || {
        public_page_within_offset(bid_page(), DEFAULT_ACCOUNT_PAGE_SIZE)
            && public_page_within_offset(proxy_page(), DEFAULT_ACCOUNT_PAGE_SIZE)
    });
    let account = use_resource(move || {
        let _ = refresh_generation();
        let _ = account_reload_generation();
        let requested = (bid_page(), proxy_page());
        let user_id = user_id.clone();
        let in_range = in_range();
        async move {
            if !in_range {
                return None;
            }
            #[cfg(test)]
            if let Some(result) =
                PageReadFixture::response((user_id.clone(), requested.0, requested.1))
            {
                return Some((requested, result));
            }
            let result = fetch_my_bids(&user_id, requested.0, requested.1).await;
            Some((requested, result))
        }
    });
    use_bid_refresh(account, in_range());

    let loaded = account.read_unchecked();
    let current = loaded
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|(requested, _)| *requested == (bid_page(), proxy_page()))
        .map(|(_, result)| result);
    // 没有当前页码的结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
    let clamp_failed = bounds.failed(PageKey::Bid) || bounds.failed(PageKey::Proxy);
    use_content_loading(match current {
        None => !clamp_failed,
        Some(Ok(page)) => page.bid_page != bid_page() || page.proxy_page != proxy_page(),
        Some(Err(_)) => false,
    });
    match current {
        _ if clamp_failed => rsx! { PublicPageClampError { bounds } },
        None => rsx! {},
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
        Some(Ok(page)) if page.bid_page != bid_page() || page.proxy_page != proxy_page() => {
            rsx! {}
        }
        Some(Ok(page)) => rsx! {
            AccountPage {
                page: page.clone(),
                bid_page: bid_page(),
                proxy_page: proxy_page(),
                on_bid_page_change: move |next_page| {
                    PageKey::Bid.write(
                        query_state_signal, ACCOUNT_PATH, next_page, QueryWriteMode::Push,
                    )
                },
                on_bid_page_clamp: move |next_page| {
                    PageKey::Bid.write(
                        query_state_signal, ACCOUNT_PATH, next_page, QueryWriteMode::Replace,
                    )
                },
                on_proxy_page_change: move |next_page| {
                    PageKey::Proxy.write(
                        query_state_signal, ACCOUNT_PATH, next_page, QueryWriteMode::Push,
                    )
                },
                on_proxy_page_clamp: move |next_page| {
                    PageKey::Proxy.write(
                        query_state_signal, ACCOUNT_PATH, next_page, QueryWriteMode::Replace,
                    )
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
    on_bid_page_change: Callback<i64, bool>,
    on_bid_page_clamp: Callback<i64, bool>,
    on_proxy_page_change: Callback<i64, bool>,
    on_proxy_page_clamp: Callback<i64, bool>,
) -> Element {
    let page_size = DEFAULT_ACCOUNT_PAGE_SIZE;
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
    on_page_change: Callback<i64, bool>,
    on_page_clamp: Callback<i64, bool>,
) -> Element {
    rsx! {
        section { class: "public-account-section", aria_labelledby: "account-bids-title",
            h2 { id: "account-bids-title", {t!("account-bids-title")} }
            if bids.is_empty() {
                p { class: "public-account-empty", {t!("account-no-bids")} }
            } else {
                div { class: "public-account-table-wrap",
                    table { class: "public-account-table",
                        aria_label: t!("account-bids-title"),
                        auction_web_shared::TableColumns {
                            layout: "public-account-bids",
                            widths: vec![12, 42, 16, 22, 8],
                        }
                        thead {
                            tr {
                                th { scope: "col", {t!("account-lot")} }
                                th { scope: "col", {t!("account-watches-equipment")} }
                                th { scope: "col", class: "numeric-column public-account-number", {t!("account-amount")} }
                                th { scope: "col", {t!("account-time")} }
                                th { scope: "col", class: "public-lot-view-cell", {t!("catalog-actions")} }
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
                report_clamp_failure: true,
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
    let lot_code = lot_code_for(&bid.equipment.category, bid.lot_number);
    rsx! {
        tr {
            class: row_class,
            // 可见的领先状态由行底色表达；同一语义以行描述提供给非视觉用户。
            "aria-description": bid_status,
            // 编号只作标识；详情经操作栏的查看按钮在页内抽屉打开。
            td { class: "public-account-code", "data-label": t!("account-lot"), "{lot_code}" }
            // 名称外没有链接承接焦点，截断时由文本自身进入 Tab 顺序以查看完整名称。
            td { "data-label": t!("account-watches-equipment"),
                TruncatedText { value: bid.equipment.name.clone(), size: "wide", portal: true }
            }
            td { class: "public-account-amount public-account-number numeric-column", "data-label": t!("account-amount"), {format_credit_amount(bid.amount)} }
            td { "data-label": t!("account-time"), Time { value: bid.created_at.clone() } }
            td { class: "public-lot-view-cell", "data-label": t!("catalog-actions"),
                LotDetailLookupButton {
                    auction_id: bid.auction_id.clone(),
                    lot_id: bid.lot_id,
                    lot_code,
                }
            }
        }
    }
}

#[component]
fn ProxySection(
    proxies: Vec<OwnedProxy>,
    page: i64,
    total: i64,
    page_size: i64,
    on_page_change: Callback<i64, bool>,
    on_page_clamp: Callback<i64, bool>,
) -> Element {
    rsx! {
        section { class: "public-account-section", aria_labelledby: "account-proxies-title",
            h2 { id: "account-proxies-title", {t!("account-proxies-title")} }
            if proxies.is_empty() {
                p { class: "public-account-empty", {t!("account-no-proxies")} }
            } else {
                div { class: "public-account-table-wrap",
                    table { class: "public-account-table",
                        aria_label: t!("account-proxies-title"),
                        auction_web_shared::TableColumns { layout: "public-account-proxies", widths: vec![10, 16, 34, 10, 10, 13, 7] }
                        thead {
                            tr {
                                th { scope: "col", {t!("account-lot")} }
                                th { scope: "col", class: "numeric-column public-account-number", {t!("account-secret-maximum")} }
                                th { scope: "col", {t!("account-transition-history")} }
                                th { scope: "col", {t!("account-mode")} }
                                th { scope: "col", {t!("account-proxy-uses")} }
                                th { scope: "col", {t!("account-status")} }
                                th { scope: "col", class: "public-lot-view-cell", {t!("catalog-actions")} }
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
                report_clamp_failure: true,
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
    let lot_code = lot_code_for(&proxy.equipment.category, proxy.lot_number);
    rsx! {
        tr {
            // 编号只作标识；详情经操作栏的查看按钮在页内抽屉打开。
            td { class: "public-account-code", "data-label": t!("account-lot"), "{lot_code}" }
            td { class: "public-account-amount public-account-number numeric-column", "data-label": t!("account-secret-maximum"), {format_credit_amount(proxy.maximum_amount)} }
            td { "data-label": t!("account-transition-history"), {transitions} }
            td { "data-label": t!("account-mode"), {proxy_mode_label(&proxy.mode)} }
            td { "data-label": t!("account-proxy-uses"), {quota} }
            td { "data-label": t!("account-status"), {proxy_status_label(&proxy.status)} }
            td { class: "public-lot-view-cell", "data-label": t!("catalog-actions"),
                LotDetailLookupButton {
                    auction_id: proxy.auction_id.clone(),
                    lot_id: proxy.lot_id,
                    lot_code,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;

    use auction_web_shared::QueryWriteMode;
    use dioxus::dioxus_core::{AttributeValue, Mutation};
    use dioxus::prelude::*;

    use super::page;
    use crate::api::{MyBidsPage, PublicSession, Worlds};
    use crate::surface::{
        PageBrowserFixture, PageReadFixture, PublicCommandOwner, page_test_contexts, settle_page,
    };

    fn authenticated_page() -> Element {
        page_test_contexts();
        let session = use_resource(|| {
            std::future::ready(Ok::<_, String>(Some(PublicSession {
                authenticated: true,
                user_id: "reader".to_owned(),
                username: "Test".to_owned(),
                expires_at: None,
                absolute_expires_at: None,
            })))
        });
        use_context_provider(|| session);
        use_context_provider(|| PublicCommandOwner(Signal::new(None)));
        page()
    }

    fn mount_account(
        browser: PageBrowserFixture,
        read: impl Fn((String, i64, i64)) -> Result<MyBidsPage, String> + 'static,
    ) -> Vec<Mutation> {
        let mut dom = VirtualDom::new(authenticated_page)
            .with_root_context(browser)
            .with_root_context(PageReadFixture::new(read))
            .with_root_context(PageReadFixture::new(|()| {
                Ok::<_, String>(Worlds { worlds: Default::default() })
            }));
        let mut edits = dom.rebuild_to_vec().edits;
        edits.extend(settle_page(&mut dom));
        edits
    }

    fn activity(bid_page: i64, proxy_page: i64, bid_total: i64) -> MyBidsPage {
        MyBidsPage {
            bids: Vec::new(),
            proxies: Vec::new(),
            bid_total,
            proxy_total: 12_000,
            page: bid_page,
            bid_page,
            proxy_page,
            page_size: 10,
        }
    }

    #[test]
    fn account_route_corrects_only_the_excess_page_and_caps_both_real_pagers() {
        for (bid, proxy, excess) in [(1001, 1002, "proxy_page"), (1002, 1001, "bid_page")] {
            let browser = PageBrowserFixture::new(
                "/me",
                &format!("bid_page={bid}&proxy_page={proxy}&keep=yes"),
            );
            let requested = Rc::new(RefCell::new(Vec::new()));
            let requests = requested.clone();
            let address = browser.clone();
            let edits = mount_account(browser.clone(), move |(user, bid_page, proxy_page)| {
                assert_eq!(user, "reader");
                assert_eq!(address.value(excess).as_deref(), Some("1001"), "先替换地址才读取");
                requests.borrow_mut().push((bid_page, proxy_page));
                Ok(activity(bid_page, proxy_page, 12_000))
            });

            let writes = browser.writes();
            assert_eq!(writes.len(), 1);
            assert_eq!(writes[0].mode, QueryWriteMode::Replace);
            assert_eq!(writes[0].keys, [excess]);
            assert!(writes[0].accepted);
            assert_eq!(&*requested.borrow(), &[(1001, 1001)]);
            assert_eq!(browser.value("keep").as_deref(), Some("yes"));
            let max_inputs = edits
                .into_iter()
                .filter_map(|edit| match edit {
                    Mutation::SetAttribute {
                        name: "max",
                        value: AttributeValue::Text(ref value),
                        id,
                        ..
                    } if value == "1001" => Some(id),
                    _ => None,
                })
                .collect::<HashSet<_>>();
            assert_eq!(max_inputs.len(), 2, "竞价与代理分页器都必须允许第 1001 页并以它为上限");
        }
    }

    #[test]
    fn accepted_total_clamp_updates_the_account_signal_and_reloads_only_the_clamped_page() {
        let browser = PageBrowserFixture::new("/me", "bid_page=3&proxy_page=4");
        let requested = Rc::new(RefCell::new(Vec::new()));
        let requests = requested.clone();
        mount_account(browser.clone(), move |(_, bid, proxy)| {
            requests.borrow_mut().push((bid, proxy));
            Ok(activity(bid, proxy, 8))
        });
        let writes = browser.writes();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].mode, QueryWriteMode::Replace);
        assert_eq!(writes[0].keys, ["bid_page"]);
        assert_eq!(browser.value("bid_page"), None);
        assert_eq!(&*requested.borrow(), &[(3, 4), (1, 4)]);
    }
}
