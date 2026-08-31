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
pub(crate) use realtime::{
    PublicConnectionState, PublicReadinessState, PublicRealtimeOwner, PublicRealtimeTarget,
    live_connection_state_name, public_connection_class, public_connection_state,
    public_connection_state_name,
};
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
    let readiness = use_context::<PublicReadinessResource>();
    let session = use_context::<PublicSessionResource>();
    let current_auction = use_context::<CurrentAuctionResource>();
    let query = read_public_query();
    let requested_auction = query.auction_id.clone();
    let historical = requested_auction.is_some();
    let historical_current = use_resource(move || {
        let _ = refresh_generation();
        let requested_auction = requested_auction.clone();
        async move {
            match requested_auction {
                Some(auction_id) => fetch_historical_auction(auction_id).await.map(Some),
                None => Ok(None),
            }
        }
    });
    let current = if historical {
        historical_current.read().clone()
    } else {
        let selected_world = selected_world();
        match current_auction.read().clone() {
            Some((loaded_world, result)) if loaded_world == selected_world => Some(result),
            _ => None,
        }
    };
    let boot_prerequisites_ready =
        readiness.read().is_some() && session.read().is_some() && current_auction.read().is_some();
    let terminal_without_auction = matches!(current.as_ref(), Some(Err(_)) | Some(Ok(None)));
    // 错误与空拍卖本身就是可操作的最终首屏；有拍卖时由 AuctionData 等待主要 lots。
    use_app_boot_ready(boot_prerequisites_ready && terminal_without_auction);

    match current.as_ref() {
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
            Placeholder {
                title: t!("auction-empty-no-auction"),
                detail: t!("auction-empty-no-auction-hint"),
            }
        },
        Some(Ok(Some(auction))) => rsx! {
            AuctionData {
                auction: auction.clone(),
                historical,
                initial_category: query.category.clone().unwrap_or_else(|| "all".to_owned()),
                initial_page: query.page,
                boot_prerequisites_ready,
            }
        },
    }
}
