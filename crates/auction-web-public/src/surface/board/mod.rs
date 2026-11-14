//! Public 当前拍卖看板的稳定出口。
//! 看板入口、世界选择、实时刷新、拍卖数据展示和预览/历史视图切换分别归档到子模块；surface
//! 仍通过本模块取得原有的组件与辅助函数。

mod data;
mod navigation;
mod realtime;
mod refresh;
mod world;

use super::*;

pub(crate) use auction_web_shared::{PageVisibility, sleep_while_visible};
pub(crate) use data::AuctionData;
pub(crate) use navigation::preview_history_switch;
pub(crate) use realtime::{
    PublicConnectionState, PublicDisplayedAuction, PublicPageRealtimeTarget, PublicReadinessState,
    PublicRealtimeOwner, PublicRealtimePageKey, PublicRealtimeTarget, TickerCountdown,
    public_connection_state, ticker_auction, ticker_countdown,
};
pub(crate) use refresh::{PublicBidRefresh, use_bid_refresh};
pub(crate) use world::{WorldPicker, supported_worlds, world_label};

#[component]
pub(crate) fn AuctionIndex() -> Element {
    // 拍品直达链接在目录当前页找不到对应行时退回的只读抽屉；宿主挂在看板之外，不随场次切换
    // 或目录重读卸载。
    let lookup = use_lot_detail_lookup();
    let mut view_request = use_context::<LotViewRequest>().0;
    use_drop(move || {
        // 离开看板即放弃尚未处置的直达请求，之后再回到看板不会突然弹出抽屉。
        if let Ok(mut request) = view_request.try_write() {
            *request = None;
        }
    });
    rsx! {
        CatalogCommandScope { AuctionBoard {} }
        LotDetailLookupHost { lookup }
    }
}

#[component]
pub(crate) fn AuctionPage() -> Element {
    redirect_public_alias("/")
}

/// 拍卖看板；首屏从同源 Public API 读取当前拍卖，不渲染示例数据。
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
    // 标题与截止时间由外壳 ticker 展示：把正文实际展示的历史场次交给它，回到当前场次或
    // 离开页面时清空。只公布与请求同一拍卖 UUID 的已加载结果；写入前用 peek 比较，不订阅自身。
    let mut displayed_auction = use_context::<PublicDisplayedAuction>().0;
    use_effect(move || {
        let next = requested_auction().and_then(|requested| {
            historical_current
                .read()
                .as_ref()
                .filter(|(loaded_id, _)| loaded_id.as_ref() == Some(&requested))
                .and_then(|(_, result)| result.as_ref().ok().cloned().flatten())
                .filter(|auction| auction.id == requested)
        });
        if *displayed_auction.peek() != next {
            displayed_auction.set(next);
        }
    });
    use_drop(move || {
        if let Ok(mut current) = displayed_auction.try_write() {
            *current = None;
        }
    });
    let historical_snapshot = historical_current.read();
    let current_snapshot = current_auction.read();
    let current = if historical {
        historical_snapshot
            .as_ref()
            .filter(|(requested, _)| requested == &requested_auction())
            .map(|(_, result)| result)
    } else {
        let selected_world = selected_world();
        current_snapshot
            .as_ref()
            .filter(|(loaded_world, _)| loaded_world == &selected_world)
            .map(|(_, result)| result)
    };
    let boot_prerequisites_ready =
        readiness.read().is_some() && session.read().is_some() && current_auction.read().is_some();
    let terminal_without_auction = matches!(current, Some(Err(_)) | Some(Ok(None)));
    // 错误与空拍卖本身就是可操作的最终首屏；有拍卖时由 AuctionData 等待主要 lots。
    use_app_boot_ready(boot_prerequisites_ready && terminal_without_auction);
    use_content_loading(current.is_none());

    match current {
        // 加载提示由 ticker 承担；正文保持空位，避免加载块被数据替换时挤压容器。
        None => rsx! {},
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
                    boot_prerequisites_ready,
                }
                // 详情抽屉挂在标记作用域内（底部标记按钮需要它），并与目录共存亡：
                // 切换场次或目录壳被错误/空态替换时，抽屉随之关闭。
                CatalogDetailHost { auction_id: auction.id.clone() }
            }
        },
    }
}
