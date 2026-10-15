//! Admin 拍卖列表页面。
//!
//! 当前页面展示 PostgreSQL 完整档的生命周期快照，并通过独立的版本化 API 提供写命令入口。

use dioxus::prelude::*;

use crate::api::fetch_admin_auctions;

use super::admin_api_error_message;
use super::query::{AuctionsQuery, use_auctions_query_state};

#[path = "auctions/actions/mod.rs"]
mod actions;
#[path = "auctions/create.rs"]
mod create;
#[path = "auctions/forum.rs"]
mod forum;
#[path = "auctions/helpers.rs"]
mod helpers;
#[path = "auctions/table.rs"]
mod table;

use table::AdminAuctionTable;

/// 加载并展示 Admin 拍卖列表。
pub(super) fn page() -> Element {
    // 世界筛选与分页跟地址栏双向同步：刷新、分享链接和浏览器前进/后退都能恢复视图。
    let query = use_auctions_query_state();
    let query_state = query.state;
    let reload_generation = use_signal(|| 0_u64);
    let auctions = use_resource(move || {
        let _ = reload_generation();
        let current = query_state();
        // 旧 Admin 拍卖页只按 world 筛选；状态不是该页面的请求维度。
        fetch_admin_auctions(current.world.clone(), "all".to_owned(), current.page)
    });

    let state = auctions
        .read_unchecked()
        .as_ref()
        .map(|result| result.clone().map_err(|error| admin_api_error_message(&error)));

    let mut on_query_change = query.clone();
    let mut on_sort_change = query.clone();
    rsx! {
        AdminAuctionTable {
            state,
            world: query_state().world.clone(),
            sort: query_state().sort.clone(),
            direction: query_state().direction,
            on_query_change: move |next: AuctionsQuery| on_query_change.push(next),
            on_sort_change: move |(sort, direction)| {
                // 排序只影响当前页展示顺序；翻页与其他筛选保持不变。
                let mut next = query_state();
                next.sort = sort;
                next.direction = direction;
                on_sort_change.push(next);
            },
            reload_generation,
        }
    }
}
