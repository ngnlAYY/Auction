//! Admin 拍卖列表页面。
//!
//! 当前页面展示 PostgreSQL 完整档的只读生命周期快照；写命令仍通过独立的版本化 API 接入。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::fetch_admin_auctions;

use super::AdminPanel;

#[path = "auctions/actions.rs"]
mod actions;
#[path = "auctions/create.rs"]
mod create;
#[path = "auctions/helpers.rs"]
mod helpers;
#[path = "auctions/table.rs"]
mod table;

use create::AdminAuctionLoadError;
use table::AdminAuctionTable;

/// 加载并展示 Admin 拍卖列表。
pub(super) fn page() -> Element {
    let world_filter = use_signal(|| "all".to_owned());
    let status_filter = use_signal(|| "all".to_owned());
    let page_number = use_signal(|| 1_i64);
    let reload_generation = use_signal(|| 0_u64);
    let auctions = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_auctions(world_filter(), status_filter(), page_number())
    });

    match &*auctions.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-auctions-title"), detail: t!("admin-auctions-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminAuctionLoadError { reload_generation }
        },
        Some(Ok(page)) => rsx! {
            div { class: "admin-auctions",
                AdminAuctionTable {
                    page: page.clone(),
                    capabilities: page.capabilities.clone(),
                    world_filter,
                    status_filter,
                    page_number,
                    reload_generation,
                }
            }
        },
    }
}
