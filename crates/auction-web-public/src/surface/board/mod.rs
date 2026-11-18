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

use auction_web_shared::{AsyncState, AsyncStateKind, QueryWriteMode, t, use_app_boot_ready};
use dioxus::prelude::*;

use self::data::{AuctionData, use_catalog_read};
use self::world::replace_world_query;
use super::loading::use_content_loading;
use super::watches::LotWatchScope;
use super::{
    AuctionParameter, CatalogCommandScope, CatalogDetailHost, CurrentAuctionResource,
    FilterParameter, LotDetailLookupHost, LotViewRequest, PageKey, PublicBoardQuery,
    PublicPageClampError, PublicReadinessResource, PublicSessionResource,
    commit_public_query_pairs, normalize_world_query, page_pathname, public_auction_home_href,
    read_public_query, redirect_public_alias_if, use_lot_detail_lookup, use_public_page_bounds,
    use_public_query_state,
};
use crate::api::{CurrentAuction, fetch_historical_auction};

pub(crate) use prefetch::{prefetch_board_lots, remember_current_auction};
pub(crate) use realtime::{
    PublicConnectionState, PublicDisplayedAuction, PublicPageRealtimeTarget, PublicReadinessState,
    PublicRealtimeOwner, PublicRealtimePageKey, PublicRealtimeTarget, TickerCountdown,
    public_connection_state, ticker_auction, ticker_countdown,
};
pub(crate) use refresh::{
    PublicBidRefresh, refresh_public_reads, use_auction_bid_refresh, use_bid_refresh,
};
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
        return rsx! { InvalidAuctionLink { world: query_state.read().world.as_deref().map(str::to_owned) } };
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

#[component]
fn InvalidCategoryLink(query: Signal<PublicBoardQuery>, values: Vec<String>) -> Element {
    let message =
        if values.len() > 1 { t!("filter-error-duplicate") } else { t!("filter-category-invalid") };
    let raw = values.join(" / ");
    rsx! {
        div { class: "ui-async-state ui-async-state--error",
            p { id: "public-catalog-category-error", role: "status", aria_live: "polite",
                "{message}"
                bdi { " {raw}" }
            }
            button {
                id: "public-catalog-clear-category",
                class: "ui-async-state__retry",
                r#type: "button",
                aria_describedby: "public-catalog-category-error",
                onclick: move |_| {
                    if page_pathname() == "/" && read_public_query().category.invalid() {
                        commit_public_query_pairs(query, &["type", "page"], &[], QueryWriteMode::Push);
                    }
                },
                {t!("filter-category-clear")}
            }
        }
    }
}

#[derive(Clone, PartialEq)]
enum HistoricalBoard {
    CorrectWorld { auction_id: String, world: String },
    Loaded(Result<Option<CurrentAuction>, String>),
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
    let requested_auction = use_memo(move || {
        let query = query_state.read();
        query.catalog_request_category()?;
        query.auction_id().map(str::to_owned)
    });
    let invalid = matches!(query_state.read().auction_parameter(), AuctionParameter::Invalid(_));
    let category_error = match &query_state.read().category {
        FilterParameter::Invalid(values) => Some(values.clone()),
        _ => None,
    };
    let query_blocked = query_state.read().catalog_request_category().is_none();
    let bounds = use_public_page_bounds(query_state, "/", |query| {
        (!matches!(query.auction_parameter(), AuctionParameter::Invalid(_))
            && !query.category.invalid())
        .then_some((PageKey::Page, query.page_size))
    });
    let historical = requested_auction().is_some();
    let historical_current = use_resource(move || {
        let _ = refresh_generation();
        let requested_auction = requested_auction();
        async move {
            let result = match requested_auction.as_ref() {
                Some(auction_id) if read_public_query().catalog_request_category().is_some() => {
                    fetch_historical_auction(auction_id.clone()).await.map(Some)
                }
                _ => Ok(None),
            };
            (requested_auction, result)
        }
    });
    let historical_board = use_memo(move || {
        let requested = requested_auction()?;
        let loaded = historical_current.read();
        let (_, result) =
            loaded.as_ref().filter(|(loaded_id, _)| loaded_id.as_ref() == Some(&requested))?;
        if let Ok(Some(auction)) = result {
            let world = normalize_world_query(&auction.world);
            let Some(world) = world.filter(|_| auction.id == requested) else {
                return Some(HistoricalBoard::Loaded(Err("invalid_response".to_owned())));
            };
            if query_state.read().world.as_deref() != Some(world.as_str()) {
                return Some(HistoricalBoard::CorrectWorld { auction_id: requested, world });
            }
            // 外壳选择器也已从地址恢复后才交付场次，避免页头和目录跨世界。
            if *selected_world.read() != world {
                return None;
            }
        }
        Some(HistoricalBoard::Loaded(result.clone()))
    });
    let mut rejected_world = use_signal(|| None::<(String, String)>);
    let correct_world = use_callback(move |(auction_id, world): (String, String)| {
        let latest = read_public_query();
        // 异步结果不能覆盖已经离开、换场或待修复的地址。
        if page_pathname() != "/"
            || latest.auction_id() != Some(auction_id.as_str())
            || latest.catalog_request_category().is_none()
            || latest.world.as_deref() == Some(world.as_str())
        {
            return;
        }
        // 写入被拒时继续使用旧地址，正文、订阅和顶栏都等待同一次纠偏成功。
        if replace_world_query(query_state, Some(&world)) {
            rejected_world.set(None);
        } else {
            rejected_world.set(Some((auction_id, world)));
        }
    });
    use_effect(move || {
        if let Some(HistoricalBoard::CorrectWorld { auction_id, world }) = historical_board() {
            let correction = (auction_id, world);
            if rejected_world.peek().as_ref() != Some(&correction) {
                correct_world.call(correction);
            }
        } else if rejected_world.peek().is_some() {
            rejected_world.set(None);
        }
    });
    let mut page_target = use_context::<PublicPageRealtimeTarget>().0;
    use_effect(move || {
        let Some(requested) = requested_auction() else { return };
        let target = match &*historical_board.read() {
            Some(HistoricalBoard::Loaded(Ok(Some(auction)))) => {
                PublicRealtimeTarget::from_auction(auction)
            }
            Some(HistoricalBoard::Loaded(_)) => {
                PublicRealtimeTarget::IdentityError { target_id: requested.clone() }
            }
            _ => PublicRealtimeTarget::Checking,
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
        let next = match historical_board() {
            Some(HistoricalBoard::Loaded(Ok(auction))) => auction,
            _ => None,
        };
        if *displayed_auction.peek() != next {
            displayed_auction.set(next);
        }
    });
    use_drop(move || {
        if let Ok(mut current) = displayed_auction.try_write() {
            *current = None;
        }
    });
    let historical_snapshot = historical_board.read();
    let current_snapshot = current_auction.read();
    let current = if query_blocked {
        None
    } else if historical {
        match historical_snapshot.as_ref() {
            Some(HistoricalBoard::Loaded(result)) => Some(result),
            _ => None,
        }
    } else {
        let selected_world = selected_world();
        current_snapshot
            .as_ref()
            .filter(|(loaded_world, _)| loaded_world == &selected_world)
            .map(|(_, result)| result)
    };
    // 目录对齐重读要等展示中的场次本轮读取也返回才判定用尽；以订阅方式读状态，读取结束即重渲染。
    let auction_reading = if historical {
        current.is_none() || *historical_current.state().read() == UseResourceState::Pending
    } else {
        *current_auction.state().read() == UseResourceState::Pending
    };
    let auction = current.and_then(|result| result.as_ref().ok()).and_then(Option::as_ref);
    let catalog_read = use_catalog_read(auction, historical, auction_reading, query_state);
    let boot_prerequisites_ready =
        readiness.read().is_some() && session.read().is_some() && current_auction.read().is_some();
    let terminal_without_auction = matches!(current, Some(Err(_)) | Some(Ok(None)));
    // 错误与空拍卖本身就是可操作的最终首屏；有拍卖时由 AuctionData 等待主要 lots。
    let clamp_failed = bounds.failed(PageKey::Page);
    let correction_failed = match historical_snapshot.as_ref() {
        Some(HistoricalBoard::CorrectWorld { auction_id, world }) => {
            rejected_world.read().as_ref().is_some_and(|(failed_id, failed_world)| {
                failed_id == auction_id && failed_world == world
            })
        }
        _ => false,
    };
    use_app_boot_ready(
        boot_prerequisites_ready
            && (terminal_without_auction
                || category_error.is_some()
                || clamp_failed
                || correction_failed),
    );
    use_content_loading(!query_blocked && current.is_none() && !correction_failed);

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
                InvalidAuctionLink { world: query_state.read().world.as_deref().map(str::to_owned) }
            } else if let Some(values) = category_error {
                InvalidCategoryLink { query: query_state, values }
            } else if clamp_failed {
                PublicPageClampError { bounds }
            } else if correction_failed {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("page-auctions-title"),
                    detail: t!("world-correction-error"),
                    retry_label: t!("data-retry"),
                    on_retry: move |_| {
                        if let Some(HistoricalBoard::CorrectWorld { auction_id, world }) = historical_board() {
                            correct_world.call((auction_id, world));
                        }
                    },
                }
            } else {
                {content}
            }
        }
    }
}
