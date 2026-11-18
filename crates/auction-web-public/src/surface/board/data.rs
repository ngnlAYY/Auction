//! Public 拍卖数据加载、倒计时和目录展示编排。

use std::cell::RefCell;
use std::rc::Rc;

use auction_time::Timestamp;
use auction_web_shared::{
    AsyncState, AsyncStateKind, BaseErrorKind, PageVisibility, QueryWriteMode,
    base_api_error_message, t, use_app_boot_ready, use_second_tick,
};
use dioxus::prelude::*;

use super::alignment::{AlignmentReread, use_alignment_reread};
use super::navigation::preview_history_switch;
use super::refresh::{refresh_public_reads, use_bid_refresh};
use crate::api::CatalogLevelFilter;
use crate::api::LotEnvelope;
use crate::api::{
    CatalogRow, CurrentAuction, LotEnvelopeRows, estimated_now, fetch_auction_list,
    fetch_current_auction, fetch_current_lots,
};
use crate::surface::loading::use_content_loading;
use crate::surface::{
    CatalogGroupFilters, CatalogTable, LotDetailLookup, LotViewDisposition, LotViewRequest,
    PublicBoardQuery, PublicSessionResource, PublicSessionView, dispose_lot_view,
    public_page_matches, update_public_page_size, update_public_query_state,
    use_catalog_proxy_snapshots,
};

/// 公开 lots 请求由场次、世界和查询参数决定。
#[derive(Clone, PartialEq)]
struct CatalogRequest {
    auction_id: String,
    world: String,
    category: String,
    equipment_group: String,
    max_level: Option<String>,
    include_unleveled: String,
    page: i64,
    page_size: i64,
}

/// 预览过渡快照按展示阶段与身份隔离；同一阶段的预览修订沿用已对齐的目录等待重读。
#[derive(Clone, PartialEq)]
struct CatalogScope {
    request: CatalogRequest,
    historical: bool,
    status: String,
    presentation_mode: String,
    bidding_open: bool,
    session_ready: bool,
    user_id: Option<String>,
}

/// 行、数量及分页来自同一次权威响应，后台更新不能混用不同修订的字段。
#[derive(Clone)]
struct CatalogSnapshot {
    page: Rc<LotEnvelope>,
    rows: Rc<Vec<CatalogRow>>,
}

impl PartialEq for CatalogSnapshot {
    fn eq(&self, other: &Self) -> bool {
        // 行与页信息一起生成；比较同一响应的引用，避免随会话展示更新逐行比较。
        Rc::ptr_eq(&self.page, &other.page)
    }
}

/// 稳定看板交给目录展示的权威结果与重试句柄。
#[derive(Clone, PartialEq)]
pub(super) struct CatalogRead {
    request: CatalogRequest,
    current: Option<Result<CatalogSnapshot, String>>,
    aligned: bool,
    alignment: AlignmentReread,
    generation: Signal<u64>,
}

/// 只在看板调用，确保身份切换保留在途 GET、目录刷新代号和已经排定的竞价补读。
pub(super) fn use_catalog_read(
    auction: Option<&CurrentAuction>,
    historical: bool,
    auction_reading: bool,
    query: Signal<PublicBoardQuery>,
) -> Option<CatalogRead> {
    #[cfg(test)]
    super::tests::observe_query(query);
    let refresh_generation = use_context::<Signal<u64>>();
    use_opening_refresh(auction, historical, refresh_generation);
    let auction_key = auction.map(|auction| (auction.id.clone(), auction.world.clone()));
    let request = use_memo(use_reactive((&auction_key,), move |(auction_key,)| {
        let (auction_id, world) = auction_key?;
        let query = query.read();
        Some(CatalogRequest {
            auction_id,
            world,
            category: query.category.clone().unwrap_or_else(|| "all".to_owned()),
            equipment_group: query.equipment_group.clone().unwrap_or_else(|| "all".to_owned()),
            max_level: query.max_level.clone(),
            include_unleveled: query.include_unleveled.clone(),
            page: query.page.max(1),
            page_size: query.page_size,
        })
    }));
    let lots_refresh_generation = use_signal(|| 0_u64);
    let lots = use_resource(move || {
        let request = request();
        let _ = refresh_generation();
        // 对齐重读只接受该次刷新代号产生的结果。
        let generation = lots_refresh_generation();
        async move {
            let request = request?;
            let category = (request.category != "all").then(|| request.category.clone());
            let equipment_group =
                (request.equipment_group != "all").then(|| request.equipment_group.clone());
            let Ok(levels) = CatalogLevelFilter::parse(
                request.max_level.as_deref().unwrap_or_default(),
                &request.include_unleveled,
            ) else {
                return Some((request, generation, Err("invalid_level_filter".to_owned())));
            };
            let result =
                read_lots(&request, category, equipment_group, levels).await.map(|mut page| {
                    CatalogSnapshot { rows: Rc::new(page.take_rows()), page: Rc::new(page) }
                });
            Some((request, generation, result))
        }
    });
    let request = request();
    use_bid_refresh(lots, request.is_some());
    let loaded = lots.read();
    // Resource 重读保留旧值。只用当前请求的快照判定场次对齐，避免越过查询或场次边界。
    let current = loaded
        .as_ref()
        .and_then(Option::as_ref)
        .filter(|(loaded_request, _, _)| Some(loaded_request) == request.as_ref());
    let aligned = match (auction, current) {
        (Some(auction), Some((_, _, Ok(snapshot)))) => {
            let page = &snapshot.page;
            page.auction_id == auction.id
                && page.auction_status == auction.status
                && page.presentation_mode == auction.presentation_mode
                && page.bidding_open == auction.bidding_open
                && page.preview_revision == auction.preview_revision
        }
        _ => true,
    };
    let alignment_key = auction.map_or_else(String::new, |auction| {
        format!(
            "{}:{}:{}:{}:{}",
            auction.id,
            auction.status,
            auction.presentation_mode,
            auction.bidding_open,
            auction.preview_revision
        )
    });
    let loaded_generation =
        current.and_then(|(_, generation, result)| result.is_ok().then_some(*generation));
    // 等场次与目录的当前读取完成，再判定是否需要重读或已用尽。
    let reading = auction_reading || *lots.state().read() == UseResourceState::Pending;
    let alignment = use_alignment_reread(
        aligned,
        alignment_key,
        loaded_generation,
        reading,
        lots_refresh_generation,
        refresh_generation,
    );
    request.map(|request| CatalogRead {
        request,
        current: current.map(|(_, _, result)| result.clone()),
        aligned,
        alignment,
        generation: lots_refresh_generation,
    })
}

fn use_opening_refresh(
    auction: Option<&CurrentAuction>,
    historical: bool,
    refresh_generation: Signal<u64>,
) {
    // 开场检查只在 effect 中读取共享秒针，目录不随每秒重渲染。
    let tick = use_second_tick();
    let visibility = use_context::<PageVisibility>();
    let should_refresh_at_open =
        !historical && auction.is_some_and(|auction| auction.presentation_mode != "live");
    let opening_at =
        auction.and_then(|auction| Timestamp::parse_from_rfc3339(&auction.opens_at).ok());
    let opening_key = auction.map(|auction| format!("{}:{}", auction.id, auction.opens_at));
    let mut opening_refresh_key = use_signal(|| None::<String>);
    let mut opening_refreshed = use_signal(|| false);
    use_effect(use_reactive(
        (&opening_key, &opening_at, &should_refresh_at_open),
        move |(opening_key, opening_at, should_refresh_at_open)| {
            let _ = tick();
            if *opening_refresh_key.peek() != opening_key {
                opening_refresh_key.set(opening_key);
                opening_refreshed.set(false);
            }
            // 开场先失效公开缓存。开场记录随看板保留，身份切换不能重复触发这次重读。
            if should_refresh_at_open
                && !*opening_refreshed.peek()
                && visibility.is_visible()
                && opening_at.is_some_and(|opens_at| estimated_now() >= opens_at)
            {
                opening_refreshed.set(true);
                refresh_public_reads(refresh_generation);
            }
        },
    ));
}

#[component]
pub(super) fn AuctionData(
    auction: CurrentAuction,
    historical: bool,
    catalog_read: CatalogRead,
    query_state_signal: Signal<PublicBoardQuery>,
    boot_prerequisites_ready: bool,
) -> Element {
    use_catalog_proxy_snapshots();
    let CatalogRead {
        request,
        current,
        aligned,
        alignment,
        generation: mut lots_refresh_generation,
    } = catalog_read;
    let current = current.as_ref();
    let refresh_generation = use_context::<Signal<u64>>();
    let mut session = use_context::<PublicSessionResource>();
    let identity = session.read();
    let session_view = PublicSessionView::from_snapshot(identity.as_ref());
    let viewer_player_id = session_view.viewer_player_id;
    let session_ready = session_view.ready;
    let session_error = session_view.error;
    let authenticated = session_view.authenticated;
    let scope = CatalogScope {
        request,
        historical,
        status: auction.status.clone(),
        presentation_mode: auction.presentation_mode.clone(),
        bidding_open: auction.bidding_open,
        session_ready,
        user_id: identity
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .and_then(Option::as_ref)
            .filter(|identity| identity.authenticated)
            .map(|identity| identity.user_id.clone()),
    };
    drop(identity);
    // 首次揭示等待目录的成功或失败结果，辅助视图切换继续局部异步。
    use_app_boot_ready(boot_prerequisites_ready && current.is_some());
    let history_switch_world = use_memo(use_reactive(
        (&auction.world, &historical, &auction.presentation_mode),
        |(world, historical, presentation_mode)| {
            (historical || presentation_mode != "live").then_some(world)
        },
    ));
    let history_switch = use_resource(move || {
        let _ = refresh_generation();
        let requested_world = history_switch_world();
        async move {
            let result = async {
                let Some(world) = requested_world.as_ref() else {
                    return Ok(None);
                };
                let current = fetch_current_auction(world.clone()).await?;
                let rounds = fetch_auction_list(world.clone()).await?;
                Ok(Some((current, rounds)))
            }
            .await;
            (requested_world, result)
        }
    });
    let alignment_exhausted = alignment.exhausted;
    let mut alignment_attempt = alignment.attempt;
    let is_live_presentation = auction.presentation_mode == "live";
    let preview_snapshot =
        use_hook(|| Rc::new(RefCell::new(None::<(CatalogScope, CatalogSnapshot)>)));
    let quiet_preview = scope.session_ready && scope.presentation_mode == "preview";
    let ready_snapshot = current.and_then(|result| result.as_ref().ok()).filter(|snapshot| {
        aligned
            && public_page_matches(
                snapshot.page.page,
                snapshot.page.page_size,
                scope.request.page,
                scope.request.page_size,
            )
    });
    // 拍品直达链接：目录当前页有这一行时由行自己打开抽屉；看板展示的不是它所在的场次，或
    // 这一页没有它时，在这里退回只读抽屉。请求只处置一次，不写 URL。
    let mut view_request = use_context::<LotViewRequest>().0;
    let lot_lookup = use_context::<LotDetailLookup>();
    let read_only_view = view_request.read().clone().filter(|target| {
        let url_world = query_state_signal.read().world.clone();
        dispose_lot_view(
            target,
            url_world.as_deref().unwrap_or("main"),
            &auction.world,
            &auction.id,
            ready_snapshot.map(|snapshot| snapshot.rows.iter().map(|row| row.lot_id.as_str())),
        ) == LotViewDisposition::ReadOnly
    });
    use_effect(use_reactive((&read_only_view,), move |(target,)| {
        if let Some(target) = target {
            view_request.set(None);
            lot_lookup.view(target.auction_id, target.lot_id);
        }
    }));
    // 只缓存当前上下文已对齐的预览；明确失败、切换上下文或离开预览立即清除。
    {
        let mut retained = preview_snapshot.borrow_mut();
        if !quiet_preview
            || retained.as_ref().is_some_and(|(retained_scope, _)| retained_scope != &scope)
            || current.is_some_and(Result::is_err)
        {
            *retained = None;
        }
        if quiet_preview && let Some(snapshot) = ready_snapshot {
            *retained = Some((scope.clone(), snapshot.clone()));
        }
    }
    let visible_snapshot = ready_snapshot.cloned().or_else(|| {
        quiet_preview
            .then(|| preview_snapshot.borrow().as_ref().map(|(_, snapshot)| snapshot.clone()))
            .flatten()
    });
    // 读取失败与对齐重读用尽都是可操作的最终状态（正文给出重试），不再占用顶栏的加载提示。
    use_content_loading(
        visible_snapshot.is_none() && !alignment_exhausted && !current.is_some_and(Result::is_err),
    );
    let catalog = match (current, visible_snapshot) {
        (Some(Err(error)), _) => {
            rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("data-error"),
                    detail: lot_list_error_message(error),
                    retry_label: t!("data-retry"),
                    // 手工重试同样经共用刷新入口：目录失败可能源于场次已经变化，场次与目录一起重读。
                    on_retry: move |_| {
                        lots_refresh_generation
                            .set(lots_refresh_generation().saturating_add(1));
                        refresh_public_reads(refresh_generation);
                    },
                }
            }
        }
        // 快照与当前场次仍不对齐，且场次与目录都已来自那次对齐重读（例如读闸繁忙时保留的旧快照）：
        // 显示可重试的错误而不是留空。重试只清掉尝试记录，由上面的 effect 经共用刷新入口重新发起
        // 一次失效后的对齐重读。
        (_, None) if alignment_exhausted => {
            rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("data-error"),
                    detail: lot_list_error_message("temporarily_unavailable"),
                    retry_label: t!("data-retry"),
                    on_retry: move |_| alignment_attempt.set(None),
                }
            }
        }
        (_, Some(snapshot)) => {
            let page = &snapshot.page;
            let fallback_page_size = scope.request.page_size;
            let page_size = page.page_size.unwrap_or(fallback_page_size);
            // 旧 lot 外壳为详情和列表共用，列表 total 可以省略且没有 has_more 字段；满页时
            // 保留一次向后探测，避免把“当前页长度”伪装成总数并永久隐藏下一页。
            let has_more = page.total.is_none()
                && i64::try_from(snapshot.rows.len()).is_ok_and(|row_count| row_count >= page_size);
            rsx! {
                CatalogTable {
                    rows: snapshot.rows.clone(),
                    projection_kind: page.projection_kind.clone(),
                    auction_status: page.auction_status.clone(),
                    category_counts: page.category_counts.clone().unwrap_or_default(),
                    active_category: scope.request.category.clone(),
                    active_equipment_group: scope.request.equipment_group.clone(),
                    level_filtered: scope.request.max_level.as_ref().is_some_and(|value| !value.is_empty())
                        || scope.request.include_unleveled != "true",
                    page_number: scope.request.page,
                    on_category_change: move |category: String| {
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.category = (category != "all").then_some(category.clone());
                                query.page = 1;
                            },
                            QueryWriteMode::Push,
                        );
                    },
                    on_page_change: move |page| {
                        update_public_query_state(
                            query_state_signal,
                            |query| query.page = page,
                            QueryWriteMode::Push,
                        );
                    },
                    on_page_clamp: move |page| {
                        update_public_query_state(
                            query_state_signal,
                            |query| query.page = page,
                            QueryWriteMode::Replace,
                        );
                    },
                    on_page_size_change: move |size| update_public_page_size(query_state_signal, size),
                    page_total: page.total,
                    page_size,
                    has_more,
                    // 可出价只看当前展示的已对齐快照：后台重读（定时轮询、竞价失效）期间保留旧值，
                    // 按钮不随请求往返消失再出现；上下文切换或阶段不一致时快照不对齐，仍失败关闭。
                    can_bid: ready_snapshot.is_some()
                        && is_live_presentation
                        && auction.bidding_open,
                    allow_slow: auction.status == "live",
                    proxy_default_mode: auction.proxy_default_mode.clone(),
                    session_ready,
                    authenticated,
                    viewer_player_id: viewer_player_id,
                }
            }
        }
        // 目录加载提示同样交给 ticker；正文不做占位，避免表格出现时挤压布局。
        _ => rsx! {},
    };
    // 切换世界或展示模式时，旧辅助结果不能冒充本次查询；同查询刷新保留已完成快照。
    let history_switch_snapshot = history_switch.read();
    let current_history_switch = history_switch_snapshot
        .as_ref()
        .filter(|(world, _)| *world == history_switch_world())
        .map(|(_, result)| result);
    // 只有会显示视图切换的预览/历史场次才等待它；竞价中的当前场次不展示切换。
    use_content_loading(
        (historical || auction.presentation_mode != "live") && current_history_switch.is_none(),
    );
    rsx! {
        // 标题与截止时间已收进顶部 ticker；场次名称只作为本区域的可访问名称。
        section {
            class: "public-auction-page",
            aria_label: t!("auction-header-title", number: auction.number),
            div { class: "public-auction-live",
                if !historical && is_live_presentation && auction.bidding_open && session_error {
                    div { class: "ui-async-state", role: "alert",
                        p { class: "ui-async-state__title", {t!("session-error")} }
                        button {
                            class: "ui-async-state__retry",
                            r#type: "button",
                            onclick: move |_| session.restart(),
                            {t!("data-retry")}
                        }
                    }
                }
                CatalogGroupFilters {
                    active_equipment_group: scope.request.equipment_group.clone(),
                    max_level: scope.request.max_level.clone(),
                    include_unleveled: scope.request.include_unleveled.clone(),
                    on_equipment_group_change: move |equipment_group: String| {
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.equipment_group = (equipment_group != "all").then_some(equipment_group);
                                query.page = 1;
                            },
                            QueryWriteMode::Push,
                        );
                    },
                    on_level_change: move |levels: CatalogLevelFilter| {
                        let max_level = levels.max_level.map(|value| value.to_string());
                        let include_unleveled = levels.include_unleveled.to_string();
                        if query_state_signal.peek().max_level == max_level
                            && query_state_signal.peek().include_unleveled == include_unleveled
                        {
                            return;
                        }
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.max_level = max_level;
                                query.include_unleveled = include_unleveled;
                                query.page = 1;
                            },
                            QueryWriteMode::Push,
                        );
                    },
                    // 预览与往期场次的视图切换停在分组栏右端。
                    {preview_history_switch(current_history_switch, &auction, historical)}
                }
                // 当前场次的目录包在一个块容器里（往期场次直接铺在弹性列上）；表格自带可访问名称。
                if !historical && (is_live_presentation || auction.presentation_mode == "preview") {
                    section { {catalog} }
                } else {
                    {catalog}
                }
            }
        }
    }
}

async fn read_lots(
    request: &CatalogRequest,
    category: Option<String>,
    equipment_group: Option<String>,
    levels: CatalogLevelFilter,
) -> Result<LotEnvelope, String> {
    #[cfg(test)]
    if let Some(reply) = super::tests::lots_request(
        &request.auction_id,
        category.as_deref(),
        request.page,
        request.page_size,
    ) {
        return reply.await;
    }
    fetch_current_lots(
        request.auction_id.clone(),
        category,
        equipment_group,
        levels,
        request.page,
        request.page_size,
    )
    .await
}

/// lot 目录读取失败的本地化文案：先处理本页特有的历史变体码，未命中走共享基础层
/// （[`base_api_error_message`]），最后落到通用兜底。
fn lot_list_error_message(error: &str) -> String {
    match error {
        "invalid_level_filter" => return t!("filter-level-invalid"),
        "request_cancelled" => return t!("error-request-cancelled"),
        "NOT_FOUND" => return t!("error-not-found"),
        "internal" | "INTERNAL" => return t!("error-internal"),
        _ => {}
    }
    base_api_error_message(error, |kind| match kind {
        BaseErrorKind::Network => t!("error-network"),
        BaseErrorKind::Timeout => t!("error-request-timeout"),
        BaseErrorKind::InvalidResponse => t!("error-invalid-response"),
        BaseErrorKind::NotFound => t!("error-not-found"),
        _ => t!("common-load-error"),
    })
    .unwrap_or_else(|| t!("common-load-error"))
}
