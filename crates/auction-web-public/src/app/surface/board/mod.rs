//! Public 当前拍卖看板的稳定出口。
//! 看板入口、世界选择、实时刷新、拍卖数据展示和预览/历史视图切换分别归档到子模块；surface
//! 仍通过本模块取得原有的组件与辅助函数。

mod data;
mod navigation;
mod realtime;
mod refresh;
mod visibility;
mod world;

use super::*;

pub(crate) use data::AuctionData;
pub(crate) use navigation::preview_history_switch;
pub(crate) use realtime::{
    PublicConnectionState, PublicPageRealtimeTarget, PublicReadinessState, PublicRealtimeOwner,
    PublicRealtimePageKey, PublicRealtimeTarget, live_connection_state_name,
    public_connection_class, public_connection_state, public_connection_state_name,
};
pub(crate) use refresh::{PublicBidRefresh, use_bid_refresh};
pub(crate) use visibility::{PageVisibility, sleep_while_visible};
pub(crate) use world::{WorldPicker, supported_worlds, world_label};

#[component]
pub(crate) fn AuctionIndex() -> Element {
    rsx! {
        CatalogCommandScope { AuctionBoard {} }
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
    let mut refresh_generation = use_context::<Signal<u64>>();
    let query_state = use_public_query_state().state;
    let readiness = use_context::<PublicReadinessResource>();
    let session = use_context::<PublicSessionResource>();
    let current_auction = use_context::<CurrentAuctionResource>();
    // 同路径的历史/当前切换也必须订阅查询，不能仅在 popstate 或首页按钮时重读。
    let requested_auction = use_memo(move || query_state.read().auction_id.clone());
    let historical = requested_auction().is_some();
    let historical_current = use_resource(move || {
        let _ = refresh_generation();
        let requested_auction = requested_auction();
        async move {
            let result = match requested_auction.as_ref() {
                Some(auction_id) => fetch_historical_auction(auction_id.clone()).await.map(Some),
                None => Ok(None),
            };
            (requested_auction, result)
        }
    });
    let mut page_target = use_context::<PublicPageRealtimeTarget>().0;
    use_effect(move || {
        let Some(requested) = requested_auction() else { return };
        let loaded = historical_current.read();
        let current = loaded
            .as_ref()
            .filter(|(loaded_id, _)| loaded_id.as_ref() == Some(&requested))
            .map(|(_, result)| result);
        let target = match current {
            Some(Ok(Some(auction))) if auction.id == requested => {
                let world = normalize_world_query(&auction.world);
                if query_state.read().world != world {
                    update_public_query_state(
                        query_state,
                        |query| query.world = world,
                        QueryWriteMode::Replace,
                    );
                }
                PublicRealtimeTarget::from_auction(auction)
            }
            Some(_) => PublicRealtimeTarget::IdentityError { target_id: requested.clone() },
            None => PublicRealtimeTarget::Checking,
        };
        let next = Some((PublicRealtimePageKey::Auction(requested), target));
        if *page_target.peek() != next {
            page_target.set(next);
        }
    });
    use_drop(move || {
        if let Ok(mut current) = page_target.try_write()
            && matches!(current.as_ref(), Some((PublicRealtimePageKey::Auction(_), _)))
        {
            *current = None;
        }
    });
    let current = if historical {
        historical_current
            .read()
            .as_ref()
            .filter(|(requested, _)| requested == &requested_auction())
            .map(|(_, result)| result.clone())
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
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("page-auctions-title"),
                detail: t!("data-loading"),
            }
        },
        Some(Err(_)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("page-auctions-title"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        },
        Some(Ok(None)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("auction-empty-no-auction"),
                detail: t!("auction-empty-no-auction-hint"),
            }
        },
        Some(Ok(Some(auction))) => rsx! {
            LotWatchScope {
                auction_id: auction.id.clone(),
                AuctionData {
                    auction: auction.clone(),
                    historical,
                    authoritative: if historical {
                        *historical_current.state().read() == UseResourceState::Ready
                    } else {
                        *current_auction.state().read() == UseResourceState::Ready
                    },
                    boot_prerequisites_ready,
                }
            }
        },
    }
}
