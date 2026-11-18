//! Public 当前拍卖看板的稳定出口。
//! 看板入口、世界选择、实时刷新、拍卖数据展示、目录与场次的对齐重读调度和预览/历史视图切换分别
//! 归档到子模块；surface
//! 仍通过本模块取得原有的组件与辅助函数。

mod alignment;
mod data;
mod navigation;
mod prefetch;
mod realtime;
mod refresh;
#[cfg(test)]
mod tests;
mod world;

use auction_web_shared::{AsyncState, AsyncStateKind, t, use_app_boot_ready};
use dioxus::prelude::*;

use self::data::{AuctionData, use_catalog_read};
use super::loading::use_content_loading;
use super::watches::LotWatchScope;
use super::{
    AuctionParameter, AuctionSelection, CatalogCommandScope, CatalogDetailHost,
    CurrentAuctionResource, LotDetailLookupHost, LotViewRequest, PublicReadinessResource,
    PublicSessionResource, commit_auction_selection, normalize_world_query,
    public_auction_home_href, read_public_query, redirect_public_alias_if, use_lot_detail_lookup,
    use_public_query_state,
};
use crate::api::fetch_historical_auction;

pub(crate) use prefetch::{prefetch_board_lots, remember_current_auction};
pub(crate) use realtime::{
    PublicConnectionState, PublicDisplayedAuction, PublicPageRealtimeTarget, PublicReadinessState,
    PublicRealtimeOwner, PublicRealtimePageKey, PublicRealtimeTarget, TickerCountdown,
    public_connection_state, ticker_auction, ticker_countdown,
};
pub(crate) use refresh::{PublicBidRefresh, refresh_public_reads, use_bid_refresh};
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
        AuctionBoard {}
        LotDetailLookupHost { lookup }
    }
}

#[component]
pub(crate) fn AuctionPage() -> Element {
    let query_state = use_public_query_state().state;
    if let AuctionParameter::Invalid(_) = query_state.read().auction_parameter() {
        return rsx! { InvalidAuctionLink { world: query_state.read().world.clone() } };
    }
    rsx! { AuctionAlias {} }
}

#[component]
fn AuctionAlias() -> Element {
    redirect_public_alias_if("/", || {
        !matches!(read_public_query().auction_parameter(), AuctionParameter::Invalid(_))
    })
}

#[component]
fn InvalidAuctionLink(world: Option<String>) -> Element {
    use_app_boot_ready(true);
    let href = public_auction_home_href(world.as_deref().unwrap_or("main"));
    rsx! {
        section { class: "ui-async-state ui-async-state--error", role: "alert",
            h2 { class: "ui-async-state__title", {t!("auction-invalid-link")} }
            p { class: "ui-async-state__detail",
                Link { to: href, {t!("auction-view-current")} }
            }
        }
    }
}

/// 拍卖看板；首屏从同源 Public API 读取当前拍卖，不渲染示例数据。
#[component]
pub(crate) fn AuctionBoard() -> Element {
    let selected_world = use_context::<Signal<String>>();
    let refresh_generation = use_context::<Signal<u64>>();
    let query_state = use_public_query_state().state;
    let readiness = use_context::<PublicReadinessResource>();
    let session = use_context::<PublicSessionResource>();
    let current_auction = use_context::<CurrentAuctionResource>();
    // 同路径的历史/当前切换也必须订阅查询，不能仅在 popstate 或首页按钮时重读。
    let requested_auction =
        use_memo(move || query_state.read().auction_parameter().selected_id().map(str::to_owned));
    let invalid = matches!(query_state.read().auction_parameter(), AuctionParameter::Invalid(_));
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
                let latest = read_public_query();
                // 异步场次响应只允许纠偏仍指向该场次的地址，非法目标必须留给用户处理。
                if latest.auction_parameter().selected_id() == Some(requested.as_str())
                    && latest.world != world
                {
                    commit_auction_selection(
                        query_state,
                        AuctionSelection::Automatic(Some(requested.clone())),
                        None,
                        |query| query.world = world,
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
    let current = if invalid {
        None
    } else if historical {
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
    // 目录对齐重读要等展示中的场次本轮读取也返回才判定用尽；以订阅方式读状态，读取结束即重渲染。
    let auction_reading = if historical {
        *historical_current.state().read() == UseResourceState::Pending
    } else {
        *current_auction.state().read() == UseResourceState::Pending
    };
    let auction = current.and_then(|result| result.as_ref().ok()).and_then(Option::as_ref);
    let catalog_read = use_catalog_read(auction, historical, auction_reading, query_state);
    let boot_prerequisites_ready =
        readiness.read().is_some() && session.read().is_some() && current_auction.read().is_some();
    let terminal_without_auction = matches!(current, Some(Err(_)) | Some(Ok(None)));
    // 错误与空拍卖本身就是可操作的最终首屏；有拍卖时由 AuctionData 等待主要 lots。
    use_app_boot_ready(boot_prerequisites_ready && terminal_without_auction);
    use_content_loading(!invalid && current.is_none());

    let content = match current {
        // 加载提示由 ticker 承担；正文保持空位，避免加载块被数据替换时挤压容器。
        None => rsx! {},
        Some(Err(_)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("page-auctions-title"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                // 与对齐重读、恢复可见共用刷新入口：先失效公开读缓存，重试才真正回到服务端。
                on_retry: move |_| refresh_public_reads(refresh_generation),
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
            if let Some(catalog_read) = catalog_read {
                LotWatchScope {
                    auction_id: auction.id.clone(),
                    AuctionData {
                        auction: auction.clone(),
                        historical,
                        catalog_read,
                        query_state_signal: query_state,
                        boot_prerequisites_ready,
                    }
                    // 详情和表格的身份投影随标记作用域重建；目录读取仍留在看板。
                    CatalogDetailHost { auction_id: auction.id.clone() }
                }
            }
        },
    };
    rsx! {
        // 场次读取临时失败或换筛选时，命令宿主仍可保留已发送命令的原样重试。
        CatalogCommandScope {
            if invalid {
                InvalidAuctionLink { world: query_state.read().world.clone() }
            } else {
                {content}
            }
        }
    }
}
