//! Public 历史、标的详情、账户和内容页面组件。

use super::*;

#[component]
pub(crate) fn AuctionList() -> Element {
    let query = read_public_query();
    let world_filter = use_signal(|| query.world.clone().unwrap_or_else(|| "all".to_owned()));
    let status_filter = use_signal(|| query.status.clone().unwrap_or_else(|| "settled".to_owned()));
    let page_number = use_signal(|| query.page.max(1));
    let refresh_generation = use_signal(|| 0_u64);
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => enabled_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    let auctions = use_resource(move || {
        let _ = refresh_generation();
        let world = world_filter();
        let status = status_filter();
        let page = page_number();
        let world = (world != "all").then_some(world);
        fetch_auction_page(world, status, page, 20)
    });
    match &*auctions.read_unchecked() {
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
        Some(Ok(page)) => rsx! {
            AuctionHistoryPage {
                page: page.clone(),
                world_filter,
                status_filter,
                page_number,
                world_options,
            }
        },
    }
}

/// 公开端错误状态；重试只重新触发当前资源读取，不在浏览器端伪造数据。
#[component]
pub(crate) fn RetryPlaceholder(
    title: String,
    detail: String,
    mut refresh_generation: Signal<u64>,
) -> Element {
    rsx! {
        section {
            class: "public-auction-empty",
            role: "alert",
            aria_labelledby: "retry-placeholder-title",
            h1 { id: "retry-placeholder-title", "{title}" }
            p { "{detail}" }
            button {
                r#type: "button",
                onclick: move |_| refresh_generation.set(refresh_generation().saturating_add(1)),
                {t!("data-retry")}
            }
        }
    }
}

#[component]
pub(crate) fn AuctionHistoryPage(
    page: AuctionListPage,
    mut world_filter: Signal<String>,
    mut status_filter: Signal<String>,
    mut page_number: Signal<i64>,
    world_options: Vec<String>,
) -> Element {
    let total = page.total.unwrap_or(page.auctions.len() as i64);
    let page_size = page.page_size.max(1);
    rsx! {
        section { class: "public-auction-page", aria_labelledby: "auction-history-title",
            h1 { id: "auction-history-title", class: "public-auction-category__title",
                {t!("page-auctions-title")}
            }
            p { class: "public-auction-page__status", {t!("page-auctions-note")} }
            div { class: "public-auction-history-controls",
                label {
                    {t!("auction-history-world")}
                    select {
                        value: world_filter(),
                        onchange: move |event| {
                            let world = event.value();
                            world_filter.set(world.clone());
                            page_number.set(1);
                            update_public_query(
                                |query| {
                                    query.world = (world != "all").then_some(world.clone());
                                    query.status = Some(status_filter());
                                    query.auction_id = None;
                                    query.category = None;
                                    query.page = 1;
                                },
                                false,
                            );
                        },
                        option { value: "all", {t!("auction-history-world-all")} }
                        for world in world_options {
                            option { value: world.clone(), {world_label(&world)} }
                        }
                    }
                }
                label {
                    {t!("auction-history-status")}
                    select {
                        value: status_filter(),
                        onchange: move |event| {
                            let status = event.value();
                            status_filter.set(status.clone());
                            page_number.set(1);
                            update_public_query(
                                |query| {
                                    query.status = Some(status.clone());
                                    query.auction_id = None;
                                    query.category = None;
                                    query.page = 1;
                                },
                                false,
                            );
                        },
                        option { value: "settled", {t!("auction-history-status-settled")} }
                        option { value: "final", {t!("auction-history-status-final")} }
                        option { value: "live", {t!("auction-history-status-live")} }
                    }
                }
            }
            if page.auctions.is_empty() {
                Placeholder { title: t!("page-auctions-title"), detail: t!("data-empty") }
            } else {
                AuctionHistoryTable { rows: page.auctions.clone() }
            }
            PublicPager {
                page: page_number(),
                page_size,
                total: Some(total),
                has_more: false,
                label: t!("catalog-pagination"),
                on_page_change: move |page| {
                    page_number.set(page);
                    update_public_query(|query| query.page = page, false);
                },
            }
        }
    }
}

#[component]
pub(crate) fn AuctionHistoryTable(rows: Vec<HistoricalAuction>) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table {
                thead {
                    tr {
                        th { {t!("auction-history-number")} }
                        th { {t!("auction-history-world")} }
                        th { {t!("auction-history-ended")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-lots")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-sold")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-unsold")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-total")} }
                        th { {t!("catalog-actions")} }
                    }
                }
                tbody {
                    for auction in rows {
                        {
                            let ended = auction.settled_at.clone()
                                .or_else(|| auction.effective_end_at.clone())
                                .or_else(|| auction.opened_at.clone())
                                .unwrap_or_else(|| auction.scheduled_open_at.clone());
                            let href = historical_history_href(&auction.id, &auction.world_id);
                            rsx! {
                                tr {
                                    td { class: "public-auction-catalog__number", "#{auction.sequence}" }
                                    td { "{auction_world_label(&auction.world_id)}" }
                                    td { time { datetime: ended.clone(), "{format_auction_time(&ended)}" } }
                                    td { class: "public-auction-catalog__number", "{auction.lot_count}" }
                                    td { class: "public-auction-catalog__number", "{auction.sold_count}" }
                                    td { class: "public-auction-catalog__number", "{auction.unsold_count}" }
                                    td { class: "public-auction-catalog__amount public-auction-catalog__number", "{format_amount(auction.sold_amount)}" }
                                    td {
                                        a { href, {t!("catalog-view")} }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn LotDetail(lot_code: String) -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let realtime_state = use_context::<Signal<LiveConnectionState>>();
    let _fallback_poll = use_future(move || async move {
        loop {
            TimeoutFuture::new(5_000).await;
            if realtime_state() != LiveConnectionState::Connected {
                refresh_generation.set(refresh_generation().saturating_add(1));
            }
        }
    });
    let lot_id = lot_code.clone();
    let detail = use_resource(move || {
        let _ = refresh_generation();
        let lot_id = lot_id.clone();
        async move { fetch_lot_detail(lot_id).await }
    });

    match &*detail.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-lot-title", code: lot_code.clone()), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-lot-title", code: lot_code.clone()),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(page)) => match page.lot.as_ref() {
            Some(LotDetailItem::Preview(lot)) => rsx! {
                PreviewLotDetailView { lot: lot.clone() }
            },
            Some(LotDetailItem::Live(lot)) => rsx! {
                LiveLotDetailView {
                    auction_id: page.auction_id.clone(),
                    auction_status: page.auction_status.clone(),
                    bidding_open: page.bidding_open,
                    lot: lot.clone(),
                }
            },
            None => rsx! {
                Placeholder { title: t!("page-lot-title", code: lot_code), detail: t!("data-empty") }
            },
        },
    }
}

#[component]
pub(crate) fn PreviewLotDetailView(lot: crate::api::PreviewLotDetail) -> Element {
    let back_href = current_auction_href();
    rsx! {
        section { class: "public-auction-lot", aria_labelledby: "lot-detail-title",
            LotDetailBack { href: back_href }
            LotDetailHeader {
                code: lot.summary.code.clone(),
                seller: lot.summary.seller_name.clone(),
                status: lot.summary.status.clone(),
            }
            div { class: "public-auction-lot__grid",
                EquipmentCard { equipment: lot.equipment.clone() }
                div { class: "public-auction-lot__bidding",
                    dl {
                        dt { {t!("catalog-start-price")} }
                        dd { class: "public-auction-lot__price", "{format_amount(lot.summary.start_price)}" }
                    }
                    p { class: "public-auction-page__status", {t!("lot-not-live")} }
                }
            }
        }
    }
}

#[component]
pub(crate) fn LiveLotDetailView(
    auction_id: String,
    auction_status: String,
    bidding_open: bool,
    lot: crate::api::LiveLot,
) -> Element {
    let back_href = current_auction_href();
    let mut clock = use_signal(estimated_now);
    let _clock_task = use_future(move || async move {
        loop {
            TimeoutFuture::new(1_000).await;
            clock.set(estimated_now());
        }
    });
    let mut refresh_generation = use_context::<Signal<u64>>();
    let session = use_resource(fetch_public_session);
    let viewer_name = match &*session.read_unchecked() {
        Some(Ok(Some(session))) => Some(session.username.clone()),
        _ => None,
    };
    let authenticated = matches!(&*session.read_unchecked(), Some(Ok(Some(_))));
    let is_leading = viewer_name
        .as_deref()
        .zip(lot.leader_name.as_deref())
        .is_some_and(|(viewer, leader)| viewer == leader);
    let mut bid_page = use_signal(|| 1_i64);
    let bid_log = use_resource({
        let auction_id = auction_id.clone();
        let lot_id = lot.id.clone();
        move || {
            let _ = refresh_generation();
            let page = bid_page();
            fetch_lot_bid_log(auction_id.clone(), lot_id.clone(), page)
        }
    });
    let countdown =
        lot.effective_end_at.as_deref().map(|ends_at| countdown_display(ends_at, clock()));

    rsx! {
        section { class: "public-auction-lot", aria_labelledby: "lot-detail-title",
            p {
                aria_live: "polite",
                class: "public-visually-hidden",
                {t!("auction-realtime-status")}
            }
            LotDetailBack { href: back_href }
            LotDetailHeader {
                code: lot_code_for(&lot.equipment.category, lot.lot_number),
                seller: lot.seller_name.clone(),
                status: lot.status.clone(),
            }
            div { class: "public-auction-lot__grid",
                EquipmentCard { equipment: lot.equipment.clone() }
                div { class: "public-auction-lot__bidding",
                    dl {
                        dt { {t!("lot-current-bid")} }
                        dd { class: "public-auction-lot__price", "{format_amount(lot.current_price)}" }
                        dt { {t!("lot-next-bid")} }
                        dd { class: "public-auction-lot__price", "{format_amount(lot.next_bid)}" }
                        dt { {t!("lot-leader")} }
                        dd {
                            if let Some(leader) = lot.leader_name.clone() {
                                CatalogParticipant {
                                    name: leader,
                                    marker: None,
                                    trailing: is_leading.then(|| t!("catalog-you")),
                                }
                            } else {
                                span { class: "public-auction-lot__none", {t!("value-empty")} }
                            }
                        }
                        dt { {t!("lot-ends-in")} }
                        dd {
                            if let (Some(ends_at), Some(countdown)) =
                                (lot.effective_end_at.clone(), countdown)
                            {
                                time { datetime: ends_at, class: countdown_class(&countdown), "{countdown.label}" }
                            } else {
                                span { class: "public-auction-lot__none", {t!("value-empty")} }
                            }
                        }
                    }
                    LiveCatalogActions {
                        lot_id: lot.id.clone(),
                        lot_code: lot_code_for(&lot.equipment.category, lot.lot_number),
                        can_bid: bidding_open && lot.status == "live",
                        current_price: lot.current_price,
                        next_bid: lot.next_bid,
                        lot_status: lot.status.clone(),
                        allow_slow: auction_status == "live",
                        authenticated,
                        leading: is_leading,
                        show_view: false,
                        show_proxy_summary: true,
                    }
                }
            }
            section { class: "public-auction-lot__history", aria_labelledby: "lot-bid-history-title",
                h2 { id: "lot-bid-history-title", {t!("lot-bid-history")} }
                match &*bid_log.read_unchecked() {
                    None => rsx! { p { role: "status", {t!("data-loading")} } },
                    Some(Err(_)) => rsx! {
                        div { class: "public-auction-page__status", role: "alert",
                            p { {t!("data-error")} }
                            button {
                                r#type: "button",
                                onclick: move |_| {
                                    refresh_generation
                                        .set(refresh_generation().saturating_add(1));
                                },
                                {t!("data-retry")}
                            }
                        }
                    },
                    Some(Ok(page)) if page.bids.is_empty() => rsx! {
                        p { class: "public-auction-lot__none", {t!("lot-no-bids")} }
                    },
                    Some(Ok(page)) => rsx! {
                        BidHistoryTable { page: page.clone() }
                        PublicPager {
                            page: bid_page(),
                            page_size: page.page_size,
                            total: page.total,
                            has_more: page.has_more.unwrap_or(false),
                            label: t!("lot-bid-history"),
                            on_page_change: move |next_page| bid_page.set(next_page),
                        }
                    },
                }
            }
        }
    }
}

#[component]
pub(crate) fn LotDetailBack(href: String) -> Element {
    rsx! {
        p { class: "public-auction-lot__back",
            a { href, "← ", {t!("lot-back-to-auction")} }
        }
    }
}

#[component]
pub(crate) fn LotDetailHeader(code: String, seller: String, status: String) -> Element {
    rsx! {
        header { class: "public-auction-lot__header",
            h1 { id: "lot-detail-title", "{code}" }
            span { class: "public-auction-status public-auction-status--{lot_status_class(&status)}", "{lot_status_label(&status)}" }
            span { class: "public-auction-lot__seller",
                {t!("catalog-seller")}, ": ", "{seller}"
            }
        }
    }
}

#[component]
pub(crate) fn EquipmentCard(equipment: EquipmentDetail) -> Element {
    let mut stats_page = use_signal(|| 1_i64);
    let stats_page_size = 10_i64;
    let current_stats_page = stats_page().max(1);
    let stats_total = i64::try_from(equipment.stats.len()).unwrap_or(i64::MAX);
    let stats_start =
        usize::try_from(current_stats_page.saturating_sub(1).saturating_mul(stats_page_size))
            .unwrap_or(usize::MAX);
    rsx! {
        div { class: "public-auction-equipment",
            h2 { "{equipment.name}" }
            dl {
                dt { {t!("equipment-category")} }
                dd { {catalog_category_label(catalog_category_key(&equipment.category))} }
                if let Some(level) = equipment.level {
                    dt { {t!("equipment-level")} }
                    dd { "{level}" }
                }
            }
            if !equipment.info_summary.is_empty() {
                p { class: "public-auction-item-summary", "{equipment.info_summary}" }
            }
            if let Some(avg_roll) = equipment.avg_roll {
                p { class: "public-auction-equipment__avg-roll",
                    span { {t!("equipment-avg-roll")} }
                    strong { "{avg_roll:.1}%" }
                }
            }
            if !equipment.stats.is_empty() {
                div { class: "public-auction-equipment__stats-wrap",
                    table { class: "public-auction-equipment__stats",
                        caption { {t!("equipment-stats")} }
                        thead {
                            tr {
                                th { {t!("equipment-stat-name")} }
                                th { {t!("equipment-stat-value")} }
                                th { {t!("equipment-stat-base")} }
                                th { {t!("equipment-stat-roll")} }
                            }
                        }
                        tbody {
                            for (name, stat) in equipment.stats.iter().skip(stats_start).take(stats_page_size as usize) {
                                tr {
                                    th { scope: "row", "{name}" }
                                    td { "{stat.value}" }
                                    td { {stat.base.map(|value| format!("{value:.2}")).unwrap_or_else(|| t!("value-empty"))} }
                                    td { {stat.roll.map(|value| format!("{value:.1}%")).unwrap_or_else(|| t!("value-empty"))} }
                                }
                            }
                        }
                    }
                }
                PublicPager {
                    page: current_stats_page,
                    page_size: stats_page_size,
                    total: Some(stats_total),
                    has_more: false,
                    label: t!("equipment-stats"),
                    on_page_change: move |next_page| stats_page.set(next_page),
                }
            }
            if !equipment.game_equipment_url.is_empty() {
                a {
                    class: "public-auction-equipment__link",
                    href: equipment.game_equipment_url,
                    target: "_blank",
                    rel: "noreferrer",
                    {t!("equipment-view-in-game")}
                }
            }
        }
    }
}

#[component]
pub(crate) fn BidHistoryTable(page: crate::api::BidLogPage) -> Element {
    rsx! {
        div { class: "public-auction-lot__table",
            table {
                thead {
                    tr {
                        th { {t!("bidlog-bidder")} }
                        th { class: "public-auction-lot__number", {t!("bidlog-amount")} }
                        th { {t!("bidlog-mechanism")} }
                        th { {t!("bidlog-time")} }
                    }
                }
                tbody {
                    for bid in page.bids {
                        tr { class: if bid.valid { "public-auction-lot__leading" } else { "" },
                            td { class: "public-auction-lot__participant-cell", "data-label": t!("bidlog-bidder"),
                                CatalogParticipant {
                                    name: bid.bidder_name.clone(),
                                    marker: (bid.mechanism == "proxy").then(|| t!("catalog-proxy-marker")),
                                    trailing: None,
                                }
                            }
                            td { class: "public-auction-lot__amount public-auction-lot__number", "data-label": t!("bidlog-amount"), "{format_amount(bid.amount)}" }
                            td { "data-label": t!("bidlog-mechanism"), "{bid_mechanism_label(&bid.mechanism)}" }
                            td { "data-label": t!("bidlog-time"), time { datetime: bid.created_at.clone(), "{format_auction_time(&bid.created_at)}" } }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn bid_mechanism_label(mechanism: &str) -> String {
    match mechanism {
        "proxy" => t!("bidlog-mechanism-proxy"),
        _ => t!("bidlog-mechanism-ordinary"),
    }
}

#[component]
pub(crate) fn BidLog() -> Element {
    reads::bid_log()
}

#[component]
pub(crate) fn History() -> Element {
    reads::history()
}

#[component]
pub(crate) fn Login() -> Element {
    session::login()
}

#[component]
pub(crate) fn Me() -> Element {
    account::page()
}

#[component]
pub(crate) fn Account() -> Element {
    redirect_public_alias("/me")
}

#[component]
pub(crate) fn Rules() -> Element {
    rules::page()
}

#[component]
pub(crate) fn Blacklist() -> Element {
    blacklist::page()
}

#[component]
pub(crate) fn CompensationPool() -> Element {
    compensation::pool_page()
}

#[component]
pub(crate) fn Compensation() -> Element {
    redirect_public_alias("/compensation-pool")
}

#[component]
pub(crate) fn Status() -> Element {
    realtime::page()
}

/// 未匹配任何已冻结路由时回到公开端首页。
///
/// 旧版 Public 对未知路径使用无历史记录跳转，而不是在外壳之外渲染独立 404；复用别名
/// 跳转边界也会保留旧 URL 中的筛选查询，避免分享链接意外丢失上下文。
#[component]
pub(crate) fn NotFound(segments: Vec<String>) -> Element {
    let _ = segments;
    redirect_public_alias("/")
}
