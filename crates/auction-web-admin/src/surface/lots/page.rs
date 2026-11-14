//! Admin lot 页面加载和错误状态。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::fetch_admin_lots;

use super::super::query::use_lots_query_state;
use super::super::{Route, admin_api_error_message};
use crate::surface::pagination::{use_admin_page, use_admin_pagination};
use crate::surface::pending::{PendingCommandPanel, use_pending_commands};

/// 加载并展示一个拍卖的 Admin lot 安全投影。
pub fn page(auction_id: String) -> Element {
    // Router 会复用相同路由组件；迭代片段按拍卖身份重挂载资源与交互状态，历史跳转不能沿用旧场次。
    rsx! {
        for auction_id in [auction_id] {
            AdminAuctionLotsPage { key: "{auction_id}", auction_id }
        }
    }
}

#[component]
fn AdminAuctionLotsPage(auction_id: String) -> Element {
    // 分页与当前页排序跟地址栏双向同步（?page=&sort=&dir=），刷新与浏览器前进/后退都能恢复。
    let query = use_lots_query_state();
    let query_state = query.state;
    let requested_page = use_memo(move || query_state.read().page);
    let reload_generation = use_signal(|| 0_u64);
    let pending = use_pending_commands(reload_generation);
    let pagination = use_admin_pagination("lots", &["page_size"], Some(100), 100_000);
    let lots = use_admin_page(
        pagination.clone(),
        move || {
            let _ = reload_generation();
            (auction_id.clone(), requested_page())
        },
        |(auction_id, page), size| fetch_admin_lots(auction_id, page, size),
    );

    let content = match &*lots.read_unchecked() {
        None => rsx! {
            AdminLotLoading {}
        },
        Some(Err(error)) => rsx! {
            AdminLotLoadError {
                detail: admin_api_error_message(error),
                reload_generation,
            }
        },
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query;
            rsx! {
                super::table::AdminLotTable {
                    page: page.clone(),
                    pagination,
                    sort: query_state().sort,
                    direction: query_state().direction,
                    on_page_change: move |next_page| {
                        let mut next = query_state();
                        next.page = next_page;
                        on_page_change.push(next);
                    },
                    on_sort_change: move |(sort, direction)| {
                        let mut next = query_state();
                        next.sort = sort;
                        next.direction = direction;
                        on_sort_change.push(next);
                    },
                    reload_generation,
                }
            }
        }
    };
    rsx! {
        PendingCommandPanel { owner: pending }
        {content}
    }
}

#[component]
fn AdminLotLoading() -> Element {
    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "auction-title",
            h1 { id: "auction-title", {t!("page-auction-lots-title", auction: "…")} }
            p { class: "admin-auction-intro",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            section { class: "admin-auction-lot-section",
                AsyncState {
                    kind: AsyncStateKind::Loading,
                    title: t!("admin-lots-loading"),
                    detail: "",
                }
            }
        }
    }
}

#[component]
fn AdminLotLoadError(detail: String, mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "auction-title",
            h1 { id: "auction-title", {t!("page-auction-lots-title", auction: "…")} }
            p { class: "admin-auction-intro",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            section { class: "admin-auction-lot-section",
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: detail,
                    detail: "",
                    retry_label: t!("admin-lots-retry"),
                    on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                }
            }
        }
    }
}
