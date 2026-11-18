//! Admin lot 页面加载和错误状态。

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

use super::super::admin_api_error_message;
use super::super::auctions::use_draft_page;
use super::super::query::use_lots_query_state;
use super::super::router::Route;
use super::edit::{AdminLotEditDialog, LotEdit};
use super::state::LotUpdateNotice;
use crate::api::fetch_admin_lots;
use crate::surface::pagination::use_admin_pagination;
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
    let pagination = use_admin_pagination("lots", &["page_size"], Some(100), 100_000);
    let draft = use_signal(|| None::<LotEdit>);
    let notice = use_signal(|| None::<LotUpdateNotice>);
    let edited_lot = use_memo(move || draft.read().as_ref().map(|edit| edit.lot_id().to_owned()))();
    let editing = edited_lot.is_some();
    let (read, lots) = use_draft_page(
        pagination.clone(),
        reload_generation,
        editing,
        move || (auction_id.clone(), requested_page()),
        |(auction_id, page), size| fetch_admin_lots(auction_id, page, size),
    );
    let visible_lots = use_memo(use_reactive((&lots,), |(lots,)| lots.and_then(Result::ok)));
    let pending = use_pending_commands(reload_generation, move |entity| {
        entity.strip_prefix("lot:").is_some_and(|lot_id| {
            visible_lots
                .peek()
                .as_ref()
                .is_none_or(|page| page.lots.iter().any(|lot| lot.id == lot_id))
        })
    });
    let latest_version = {
        let page = lots.as_ref().and_then(|result| result.as_ref().ok());
        edited_lot.as_ref().and_then(|lot_id| {
            page.and_then(|page| page.lots.iter().find(|lot| &lot.id == lot_id))
                .map(|lot| lot.state_version)
        })
    };

    let content = match lots {
        None => rsx! {
            AdminLotLoading {}
        },
        Some(Err(error)) => rsx! {
            AdminLotLoadError {
                detail: admin_api_error_message(&error),
                reload_generation,
            }
        },
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query;
            rsx! {
                super::table::AdminLotTable {
                    page: page.as_ref().clone(),
                    pagination,
                    edits_ready: read.is_ready(),
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
                    draft,
                    notice,
                }
            }
        }
    };
    let pending_in_dialog = edited_lot.as_ref().is_some_and(|id| {
        let entity = format!("lot:{id}");
        pending.contains(&entity) && !pending.is_busy(&entity)
    });
    rsx! {
        if !pending_in_dialog {
            PendingCommandPanel { owner: pending }
        }
        {content}
        if editing {
            AdminLotEditDialog { draft, latest_version, read, reload_generation, notice }
        }
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
            section {
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
            section {
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
