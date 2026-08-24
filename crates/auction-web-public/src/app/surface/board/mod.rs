//! Public 当前拍卖看板的稳定出口。
//!
//! 看板入口、世界选择、实时刷新、拍卖数据展示和历史导航分别归档到子模块；surface
//! 仍通过本模块取得原有的组件与辅助函数。

mod data;
mod navigation;
mod realtime;
mod world;

use super::*;

pub(crate) use data::AuctionData;
pub(crate) use navigation::{HistoricalAuctionNavigation, preview_history_switch};
pub(crate) use realtime::{live_connection_class, should_refresh_auction_board};
pub(crate) use world::{WorldPicker, enabled_worlds, world_label};

#[component]
pub(crate) fn AuctionIndex() -> Element {
    rsx! {
        AuctionBoard {}
    }
}

#[component]
pub(crate) fn AuctionPage() -> Element {
    redirect_public_alias("/")
}

/// 拍卖看板；首屏从同源 Public API 读取当前拍卖，不再渲染迁移阶段示例数据。
#[component]
pub(crate) fn AuctionBoard() -> Element {
    let selected_world = use_context::<Signal<String>>();
    let refresh_generation = use_context::<Signal<u64>>();
    let query = read_public_query();
    let requested_auction = query.auction_id.clone();
    let historical = requested_auction.is_some();
    let current = use_resource(move || {
        let _ = refresh_generation();
        let world = selected_world();
        let requested_auction = requested_auction.clone();
        async move {
            match requested_auction {
                Some(auction_id) => fetch_historical_auction(auction_id).await.map(Some),
                None => fetch_current_auction(world).await,
            }
        }
    });

    match &*current.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-auctions-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(None)) => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-empty") }
        },
        Some(Ok(Some(auction))) => rsx! {
            AuctionData {
                auction: auction.clone(),
                historical,
                initial_category: query.category.clone().unwrap_or_else(|| "all".to_owned()),
                initial_page: query.page,
            }
        },
    }
}
