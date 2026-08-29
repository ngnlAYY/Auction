//! Admin 拍卖列表页面。
//!
//! 当前页面展示 PostgreSQL 完整档的生命周期快照，并通过独立的版本化 API 提供写命令入口。

use dioxus::prelude::*;

use crate::api::fetch_admin_auctions;

use super::admin_api_error_message;

#[path = "auctions/actions/mod.rs"]
mod actions;
#[path = "auctions/create.rs"]
mod create;
#[path = "auctions/helpers.rs"]
mod helpers;
#[path = "auctions/table.rs"]
mod table;

use table::AdminAuctionTable;

/// 加载并展示 Admin 拍卖列表。
pub(super) fn page() -> Element {
    let world_filter = use_signal(String::new);
    let page_number = use_signal(|| 1_i64);
    let reload_generation = use_signal(|| 0_u64);
    let auctions = use_resource(move || {
        let _ = reload_generation();
        // 旧 Admin 拍卖页只按 world 筛选；状态不是该页面的请求维度。
        fetch_admin_auctions(world_filter(), "all".to_owned(), page_number())
    });

    let state = auctions
        .read_unchecked()
        .as_ref()
        .map(|result| result.clone().map_err(|error| admin_api_error_message(&error)));

    rsx! {
        AdminAuctionTable {
            state,
            world_filter,
            page_number,
            reload_generation,
        }
    }
}
