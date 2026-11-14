//! Admin 拍卖列表页面。
//! 当前页面展示 PostgreSQL 完整档的生命周期快照，并通过独立的版本化 API 提供写命令入口。

use dioxus::prelude::*;

use super::admin_api_error_message;
use super::pagination::{use_admin_page, use_admin_pagination};
use super::pending::{PendingCommandPanel, use_pending_commands};
use super::query::{AuctionsQuery, use_auctions_query_state};
use crate::api::fetch_admin_auctions;

mod actions;
mod create;
mod forum;
mod presentation;
mod table;

use table::AdminAuctionTable;

/// 加载并展示 Admin 拍卖列表。
pub(super) fn page() -> Element {
    // 世界筛选与分页跟地址栏双向同步：刷新、分享链接和浏览器前进/后退都能恢复视图。
    let query = use_auctions_query_state();
    let query_state = query.state;
    // 请求只订阅服务端筛选和页码；本地排序变化不能重发列表请求。
    let requested_query = use_memo(move || {
        let current = query_state.read();
        (current.world.clone(), current.page)
    });
    let reload_generation = use_signal(|| 0_u64);
    let pending = use_pending_commands(reload_generation);
    let pagination = use_admin_pagination("auctions", &["page_size"], Some(100), 100_000);
    let auctions = use_admin_page(
        pagination.clone(),
        move || {
            let _ = reload_generation();
            requested_query()
        },
        |(world, page), size| fetch_admin_auctions(world, "all".to_owned(), page, size),
    );

    let state = auctions
        .read_unchecked()
        .as_ref()
        .map(|result| result.clone().map_err(|error| admin_api_error_message(&error)));

    let mut on_query_change = query.clone();
    let mut on_sort_change = query;
    rsx! {
        PendingCommandPanel { owner: pending }
        AdminAuctionTable {
            state,
            pagination,
            world: query_state().world,
            sort: query_state().sort,
            direction: query_state().direction,
            on_query_change: move |next: AuctionsQuery| on_query_change.push(next),
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
