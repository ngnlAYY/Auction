//! Public 拍卖数据加载、倒计时和目录展示编排。

use super::*;
use crate::api::CatalogLevelFilter;
use crate::api::LotEnvelope;

/// 目录读取与展示的上下文；预览修订号不属于上下文，允许同场后台更新时保留已确认目录。
#[derive(Clone, PartialEq)]
struct CatalogScope {
    auction_id: String,
    world: String,
    historical: bool,
    status: String,
    presentation_mode: String,
    bidding_open: bool,
    session_ready: bool,
    user_id: Option<String>,
    category: String,
    equipment_group: String,
    max_level: Option<String>,
    include_unleveled: String,
    page: i64,
    page_size: i64,
}

/// 行、数量及分页来自同一次权威响应，后台更新不能混用不同修订的字段。
#[derive(Clone)]
struct CatalogSnapshot {
    page: Rc<LotEnvelope>,
    rows: Rc<Vec<CatalogRow>>,
}

#[component]
pub(crate) fn AuctionData(
    auction: CurrentAuction,
    historical: bool,
    authoritative: bool,
    boot_prerequisites_ready: bool,
) -> Element {
    // 开场检查只在 effect 中读取共享秒针；倒计时由叶子组件订阅，目录不随每秒重渲染。
    let tick = use_second_tick();
    // 装备分组、分类与页码与地址栏双向同步：初值取自 URL，浏览器前进/后退由 hook 恢复。
    let query_state = use_public_query_state();
    // Signal 为 Copy，各字段 memo 共享同一状态信号，只在各自字段变化时通知订阅者。
    // 借用查询后只克隆所需字段，避免每个派生值先复制整份筛选与历史查询。
    let query_state_signal = query_state.state;
    let active_category = use_memo(move || {
        query_state_signal.read().category.clone().unwrap_or_else(|| "all".to_owned())
    });
    let active_equipment_group = use_memo(move || {
        query_state_signal.read().equipment_group.clone().unwrap_or_else(|| "all".to_owned())
    });
    let active_max_level = use_memo(move || query_state_signal.read().max_level.clone());
    let active_include_unleveled =
        use_memo(move || query_state_signal.read().include_unleveled.clone());
    let page_number = use_memo(move || query_state_signal.read().page.max(1));
    let requested_page_size = use_memo(move || query_state_signal.read().page_size);
    let visibility = use_context::<PageVisibility>();
    let mut refresh_generation = use_context::<Signal<u64>>();
    let should_refresh_at_open = !historical && auction.presentation_mode != "live";
    let opening_at =
        DateTime::parse_from_rfc3339(&auction.opens_at).ok().map(|value| value.with_timezone(&Utc));
    let opening_key = format!("{}:{}", auction.id, auction.opens_at);
    let mut opening_refresh_key = use_signal(|| None::<String>);
    let mut opening_refreshed = use_signal(|| false);
    use_effect(move || {
        let _ = tick();
        let now = estimated_now();
        if *opening_refresh_key.peek() != Some(opening_key.clone()) {
            opening_refresh_key.set(Some(opening_key.clone()));
            opening_refreshed.set(false);
        }
        // 预览不建立 WebSocket，不能用 Live 连接态作为开场补读前提；这里只失效权威读取。
        if should_refresh_at_open
            && !*opening_refreshed.peek()
            && visibility.is_visible()
            && opening_at.is_some_and(|opens_at| now >= opens_at)
        {
            opening_refreshed.set(true);
            let next_generation = refresh_generation.peek().saturating_add(1);
            refresh_generation.set(next_generation);
        }
    });
    let mut session = use_context::<PublicSessionResource>();
    let session_view = PublicSessionView::from_snapshot(session.read().as_ref());
    let viewer_player_id = session_view.viewer_player_id.clone();
    let session_ready = session_view.ready;
    let session_error = session_view.error;
    let authenticated = session_view.authenticated;
    let catalog_scope =
        use_memo(use_reactive((&auction, &historical), move |(auction, historical)| {
            let identity = session.read();
            let identity = identity.as_ref().and_then(|result| result.as_ref().ok());
            CatalogScope {
                auction_id: auction.id,
                world: auction.world,
                historical,
                status: auction.status,
                presentation_mode: auction.presentation_mode,
                bidding_open: auction.bidding_open,
                session_ready: identity.is_some(),
                user_id: identity
                    .and_then(Option::as_ref)
                    .filter(|identity| identity.authenticated)
                    .map(|identity| identity.user_id.clone()),
                category: active_category(),
                equipment_group: active_equipment_group(),
                max_level: active_max_level(),
                include_unleveled: active_include_unleveled(),
                page: page_number(),
                page_size: requested_page_size(),
            }
        }));
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
    let mut lots_refresh_generation = use_signal(|| 0_u64);
    let lots = use_resource(move || {
        let _ = refresh_generation();
        let _ = lots_refresh_generation();
        let scope = catalog_scope();
        async move {
            let category = (scope.category != "all").then(|| scope.category.clone());
            let equipment_group =
                (scope.equipment_group != "all").then(|| scope.equipment_group.clone());
            let Ok(levels) = CatalogLevelFilter::parse(
                scope.max_level.as_deref().unwrap_or_default(),
                &scope.include_unleveled,
            ) else {
                return (scope, Err("invalid_level_filter".to_owned()));
            };
            let result = fetch_current_lots(
                scope.auction_id.clone(),
                category,
                equipment_group,
                levels,
                scope.page,
                scope.page_size,
            )
            .await
            .map(|mut page| CatalogSnapshot {
                rows: Rc::new(page.take_rows()),
                page: Rc::new(page),
            });
            (scope, result)
        }
    });
    use_bid_refresh(lots, true);
    let scope = catalog_scope();
    let loaded = lots.read();
    // Resource 重读会保留旧值，先核对请求上下文，避免跨账号、世界、筛选或阶段复用。
    let current = loaded.as_ref().filter(|(loaded_scope, _)| loaded_scope == &scope);
    let aligned = match current {
        Some((_, Ok(snapshot))) => {
            let page = &snapshot.page;
            page.auction_id == auction.id
                && page.auction_status == auction.status
                && page.presentation_mode == auction.presentation_mode
                && page.bidding_open == auction.bidding_open
                && page.preview_revision == auction.preview_revision
        }
        _ => true,
    };
    // 首次揭示要等主要目录已有成功或失败结果；辅助视图切换继续局部异步，避免拖慢整页。
    use_app_boot_ready(boot_prerequisites_ready && current.is_some());
    let alignment_key = format!(
        "{}:{}:{}:{}:{}",
        auction.id,
        auction.status,
        auction.presentation_mode,
        auction.bidding_open,
        auction.preview_revision
    );
    let mut alignment_attempt = use_signal(|| None::<String>);
    use_effect(use_reactive((&aligned, &alignment_key), move |(aligned, alignment_key)| {
        if aligned {
            if alignment_attempt().is_some() {
                alignment_attempt.set(None);
            }
        } else if alignment_attempt().as_deref() != Some(alignment_key.as_str()) {
            alignment_attempt.set(Some(alignment_key.clone()));
            lots_refresh_generation.set(lots_refresh_generation().saturating_add(1));
        }
    }));
    let status_label = auction_status_label(&auction.status);
    let is_live_presentation = auction.presentation_mode == "live";
    let is_opening_presentation = auction.presentation_mode == "opening";
    let show_live_countdown =
        is_live_presentation && matches!(auction.status.as_str(), "live" | "final");
    let auction_end = if show_live_countdown {
        auction.scheduled_ends_at.clone()
    } else if is_opening_presentation {
        None
    } else if !is_live_presentation {
        Some(auction.opens_at.clone())
    } else {
        // settling/settled 已进入终态收口，不再把过去的计划截止时间渲染为“落锤中…”。
        None
    };
    let preview_snapshot =
        use_hook(|| Rc::new(RefCell::new(None::<(CatalogScope, CatalogSnapshot)>)));
    let quiet_preview = scope.session_ready && scope.presentation_mode == "preview";
    let ready_snapshot = current.and_then(|(_, result)| result.as_ref().ok()).filter(|snapshot| {
        aligned
            && public_page_matches(
                snapshot.page.page,
                snapshot.page.page_size,
                scope.page,
                scope.page_size,
            )
    });
    // 只缓存当前上下文已对齐的预览；明确失败、切换上下文或离开预览立即清除。
    {
        let mut retained = preview_snapshot.borrow_mut();
        if !quiet_preview
            || retained.as_ref().is_some_and(|(retained_scope, _)| retained_scope != &scope)
            || current.is_some_and(|(_, result)| result.is_err())
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
    let catalog = match (current, visible_snapshot) {
        (Some((_, Err(error))), _) => {
            rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("data-error"),
                    detail: lot_list_error_message(error),
                    retry_label: t!("data-retry"),
                    on_retry: move |_| {
                        lots_refresh_generation
                            .set(lots_refresh_generation().saturating_add(1));
                    },
                }
            }
        }
        (_, Some(snapshot)) => {
            let page = &snapshot.page;
            let page_size = page.page_size.unwrap_or(requested_page_size());
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
                    active_category: active_category(),
                    active_equipment_group: active_equipment_group(),
                    level_filtered: scope.max_level.as_ref().is_some_and(|value| !value.is_empty())
                        || scope.include_unleveled != "true",
                    page_number: page_number(),
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
                    can_bid: authoritative
                        && *lots.state().read() == UseResourceState::Ready
                        && is_live_presentation
                        && auction.bidding_open,
                    allow_slow: auction.status == "live",
                    session_ready,
                    authenticated,
                    viewer_player_id: viewer_player_id.clone(),
                }
            }
        }
        _ => rsx! { p { class: "public-page-status", role: "status", {t!("common-loading")} } },
    };
    // 切换世界或展示模式时，旧辅助结果不能冒充本次查询；同查询刷新保留已完成快照。
    let history_switch_snapshot = history_switch.read();
    let current_history_switch = history_switch_snapshot
        .as_ref()
        .filter(|(world, _)| *world == history_switch_world())
        .map(|(_, result)| result);

    rsx! {
        section { class: "public-auction-page", aria_labelledby: "auction-title",
            div { class: "public-auction-live",
                header { class: "public-auction-round",
                    div { class: "public-auction-round__title",
                        h1 {
                            id: "auction-title",
                            {t!("auction-header-title", number: auction.number)}
                        }
                        span { class: "public-auction-round__world", "{auction_world_label(&auction.world)}" }
                        span {
                            class: "public-auction-round__status public-auction-round__status--{auction_status_class(&auction.status)}",
                            "{status_label}"
                        }
                    }
                    if is_opening_presentation {
                        p { class: "public-auction-round__end",
                            {t!("auction-header-opening")}
                        }
                    } else if let Some(scheduled_ends_at) = auction_end.clone() {
                        AuctionHeaderCountdown {
                            scheduled_ends_at,
                            is_live_presentation,
                            status: auction.status.clone(),
                        }
                    }
                    {preview_history_switch(current_history_switch, &auction, historical)}
                }
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
                    active_equipment_group: active_equipment_group(),
                    max_level: active_max_level(),
                    include_unleveled: active_include_unleveled(),
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
                }
                if !historical && is_live_presentation {
                    section { aria_labelledby: "auction-lots",
                        h2 { id: "auction-lots", class: "public-visually-hidden",
                            {t!("auction-lots")}
                        }
                        {catalog}
                    }
                } else if !historical && auction.presentation_mode == "preview" {
                    section { aria_labelledby: "auction-preview-lots",
                        h2 { id: "auction-preview-lots", class: "public-visually-hidden",
                            {t!("auction-preview-lots")}
                        }
                        {catalog}
                    }
                } else {
                    {catalog}
                }
            }
        }
    }
}

/// 拍卖头部的开场/结束倒计时。秒级节拍只在本叶子组件内订阅：顶层 [`AuctionData`]
/// 不再持有每秒变化的时钟信号，目录子树不随秒针重渲染。
#[component]
fn AuctionHeaderCountdown(
    scheduled_ends_at: String,
    is_live_presentation: bool,
    status: String,
) -> Element {
    let tick = use_second_tick();
    let _ = tick();
    let countdown = countdown_display(&scheduled_ends_at, estimated_now());
    rsx! {
        p { class: "public-auction-round__end",
            if is_live_presentation {
                if status == "final" {
                    {t!("auction-header-final-ends-in")}
                } else {
                    {t!("auction-header-ends-in")}
                }
            } else {
                {t!("auction-header-opens-in")}
            }
            span {
                class: countdown_class(&countdown),
                aria_label: t!("auction-countdown"),
                "{countdown.label}"
            }
            Time { value: scheduled_ends_at.clone() }
        }
    }
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
