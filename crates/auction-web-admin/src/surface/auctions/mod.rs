//! Admin 拍卖列表页面。
//! 当前页面展示服务端的拍卖生命周期快照，并通过独立的版本化 API 提供写命令入口；未配置
//! 数据库的部署由服务端返回能力不可用（`feature_not_supported`）。

use dioxus::prelude::*;

use super::admin_api_error_message;
use super::pagination::use_admin_pagination;
use super::pending::{PendingCommandPanel, use_pending_commands};
use super::query::{AuctionsQuery, use_auctions_query_state};
use crate::api::fetch_admin_auctions;

mod actions;
mod create;
pub(in crate::surface) mod editor;
mod forum;
mod presentation;
mod table;

use actions::{AdminAuctionDraftDialogs, AdminAuctionNotice, use_auction_drafts};
pub(super) use editor::use_draft_page;
use table::AdminAuctionTable;

fn invalidates_auction_list(entity: &str) -> bool {
    entity == "create-auction" || entity.starts_with("auction:") || entity.starts_with("intake:")
}

/// 加载并展示 Admin 拍卖列表。
pub(super) fn page() -> Element {
    // 世界筛选与分页跟地址栏双向同步：刷新、分享链接和浏览器前进/后退都能恢复视图。
    let query = use_auctions_query_state();
    let query_state = query.state;
    let mut reload_generation = use_signal(|| 0_u64);
    let pending = use_pending_commands(reload_generation, invalidates_auction_list);
    let pagination = use_admin_pagination("auctions", &["page_size"], Some(100), 100_000);
    let drafts = use_auction_drafts();
    let notice = use_signal(|| None::<AdminAuctionNotice>);
    let edited_auction = use_memo(move || drafts.auction_id())();
    let (read, auctions) = use_draft_page(
        pagination.clone(),
        reload_generation,
        edited_auction.is_some(),
        move || {
            // 只把服务端筛选和页码作为请求身份；本地排序变化不能重发列表请求。
            let current = query_state.read();
            (current.world.clone(), current.page)
        },
        |(world, page), size| fetch_admin_auctions(world, page, size),
    );
    let page = auctions.as_ref().and_then(|result| result.as_ref().ok());
    let latest_version = edited_auction
        .as_ref()
        .and_then(|auction_id| {
            page.and_then(|page| page.auctions.iter().find(|auction| &auction.id == auction_id))
        })
        .map(|auction| auction.state_version);
    let state = auctions.map(|result| {
        result.map(|page| page.as_ref().clone()).map_err(|code| admin_api_error_message(&code))
    });

    let mut on_query_change = query.clone();
    let mut on_sort_change = query;
    let pending_in_dialog = edited_auction.as_ref().is_some_and(|id| {
        let entity = format!("auction:{id}");
        pending.contains(&entity) && !pending.is_busy(&entity)
    });
    rsx! {
        if !pending_in_dialog {
            PendingCommandPanel { owner: pending }
        }
        AdminAuctionTable {
            state,
            pagination,
            edits_ready: read.is_ready(),
            world: query_state().world,
            sort: query_state().sort,
            direction: query_state().direction,
            on_query_change: move |next: AuctionsQuery| on_query_change.push(next),
            on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
            on_sort_change: move |(sort, direction)| {
                let mut next = query_state();
                next.sort = sort;
                next.direction = direction;
                on_sort_change.push(next);
            },
            notice,
        }
        if edited_auction.is_some() {
            AdminAuctionDraftDialogs { drafts, latest_version, read, notice, reload_generation }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::invalidates_auction_list;

    #[test]
    fn auction_list_reloads_for_intake_in_either_world() {
        assert!(invalidates_auction_list("intake:main"));
        assert!(invalidates_auction_list("intake:isekai"));
        assert!(invalidates_auction_list("auction:0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b"));
        assert!(!invalidates_auction_list("lot:00000000-0000-4000-8000-000000000023"));
    }
}
