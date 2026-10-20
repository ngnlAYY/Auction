//! Public 拍卖目录筛选、分组表格和分页。

use super::*;
use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::format_credit_amount_full;

#[component]
pub(crate) fn CatalogTable(
    rows: Rc<Vec<CatalogRow>>,
    projection_kind: String,
    auction_status: String,
    category_counts: BTreeMap<String, i64>,
    active_category: String,
    page_number: i64,
    on_category_change: EventHandler<String>,
    on_page_change: EventHandler<i64>,
    on_page_size_change: EventHandler<i64>,
    page_total: Option<i64>,
    page_size: i64,
    has_more: bool,
    can_bid: bool,
    allow_slow: bool,
    session_ready: bool,
    authenticated: bool,
    viewer_player_id: Option<String>,
) -> Element {
    let active = active_category.clone();
    let groups = grouped_catalog_rows(&rows, &active);
    let is_live = projection_kind == "live";
    // 历史成交目录仍使用 live 价格投影；是否保留操作列单独按拍卖阶段决定。
    let show_actions = auction_status != "settled";
    let all_count = catalog_count(&category_counts, "all", &rows);

    rsx! {
        nav { class: "public-auction-filter", aria_label: t!("filter-label"),
            button {
                class: "public-auction-filter__tab",
                r#type: "button",
                // 筛选 tab 表达的是“当前视图”而非开关态，语义上用 aria-current 而非 aria-pressed。
                aria_current: if active == "all" { "true" } else { "false" },
                onclick: move |_| on_category_change.call("all".to_owned()),
                {t!("filter-all")}
                span { class: "public-auction-filter__count", "{all_count}" }
            }
            for (category, _) in CATALOG_CATEGORIES {
                button {
                    class: "public-auction-filter__tab",
                    r#type: "button",
                    aria_current: if active == category { "true" } else { "false" },
                    onclick: move |_| on_category_change.call(category.to_owned()),
                    {catalog_category_label(category)}
                    span { class: "public-auction-filter__count", "{catalog_count(&category_counts, category, &rows)}" }
                }
            }
        }
        if groups.is_empty() {
            div { class: "ui-empty-state", role: "status",
                p { class: "ui-empty-state__title",
                    if active == "all" {
                        {t!("catalog-empty-all")}
                    } else {
                        {t!("catalog-empty-category")}
                    }
                }
            }
        }
        for (category, indices) in groups {
            section { class: "public-auction-category",
                h3 { class: "public-auction-category__title",
                    {catalog_category_label(category)}
                    span { class: "public-auction-category__code", "{catalog_category_code(category)}" }
                }
                if is_live {
                    LiveCatalogTable {
                        rows: Rc::clone(&rows),
                        indices,
                        show_actions,
                        can_bid,
                        allow_slow,
                        session_ready,
                        authenticated,
                        viewer_player_id: viewer_player_id.clone(),
                    }
                } else {
                    PreviewCatalogTable { rows: Rc::clone(&rows), indices }
                }
            }
        }
        PublicPager {
            page: page_number,
            page_size,
            total: page_total,
            has_more,
            label: t!("auction-pager-label"),
            on_page_change,
            on_page_size_change,
        }
    }
}
#[component]
pub(crate) fn LiveCatalogTable(
    rows: Rc<Vec<CatalogRow>>,
    indices: Vec<usize>,
    show_actions: bool,
    can_bid: bool,
    allow_slow: bool,
    session_ready: bool,
    authenticated: bool,
    viewer_player_id: Option<String>,
) -> Element {
    // 语义化 colgroup 分配完整百分比宽度；窄屏仍保留表头与行列，不切换为卡片。
    let (layout, widths) = if show_actions {
        ("public-catalog-live", vec![6, 22, 5, 4, 4, 6, 8, 8, 6, 7, 6, 18])
    } else {
        ("public-catalog-settled", vec![7, 30, 6, 5, 5, 8, 10, 9, 8, 6, 6])
    };
    rsx! {
        div { class: "public-auction-catalog",
            table { class: "public-auction-catalog__table public-auction-catalog__table--live ui-table--responsive",
                caption { class: "public-visually-hidden", {t!("auction-lots")} }
                auction_web_shared::TableColumns { layout, widths }
                thead {
                    tr {
                        th { scope: "col", {t!("catalog-code")} }
                        th { scope: "col", {t!("catalog-item")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("equipment-avg-roll")} }
                        th { scope: "col", {t!("catalog-level")} }
                        th { scope: "col", {t!("catalog-primary-attributes")} }
                        th { scope: "col", {t!("catalog-seller")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("catalog-current")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("catalog-next-bid")} }
                        th { scope: "col", {t!("catalog-leader")} }
                        th { scope: "col", {t!("catalog-remaining")} }
                        th { scope: "col", {t!("catalog-status")} }
                        if show_actions {
                            th { scope: "col", {t!("catalog-actions")} }
                        }
                    }
                }
                tbody {
                    for index in indices {
                        LiveCatalogRow {
                            // 以 lot_id 作为稳定 key，避免刷新后行状态错位绑定到别的 lot。
                            key: "{rows[index].lot_id}",
                            row: rows[index].clone(),
                            show_actions,
                            can_bid,
                            allow_slow,
                            session_ready,
                            authenticated,
                            viewer_player_id: viewer_player_id.clone(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn PreviewCatalogTable(rows: Rc<Vec<CatalogRow>>, indices: Vec<usize>) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table { class: "public-auction-catalog__table public-auction-catalog__table--preview ui-table--responsive",
                caption { class: "public-visually-hidden", {t!("auction-preview-lots")} }
                // 预览目录不提供行内操作；colgroup、表头与行同时保留十列，编号仍可进入详情。
                auction_web_shared::TableColumns { layout: "public-catalog-preview", widths: vec![6, 36, 7, 5, 7, 13, 12, 4, 4, 6] }
                thead {
                    tr {
                        th { scope: "col", {t!("catalog-code")} }
                        th { scope: "col", {t!("catalog-item")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("equipment-avg-roll")} }
                        th { scope: "col", {t!("catalog-level")} }
                        th { scope: "col", {t!("catalog-primary-attributes")} }
                        th { scope: "col", {t!("catalog-seller")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("catalog-start-price")} }
                        th { scope: "col", {t!("catalog-leader")} }
                        th { scope: "col", {t!("catalog-remaining")} }
                        th { scope: "col", {t!("catalog-status")} }
                    }
                }
                tbody {
                    for index in indices {
                        // 以 lot_id 作为稳定 key，避免刷新后行状态错位绑定到别的 lot。
                        PreviewCatalogRow { key: "{rows[index].lot_id}", row: rows[index].clone() }
                    }
                }
            }
        }
    }
}

/// 行内剩余时间倒计时。秒级节拍只在本叶子组件内订阅：整行的装备、价格与操作单元格
/// 不随秒针重渲染，只有真正显示倒计时的单元格每秒重算。
#[component]
fn LiveRowCountdown(remaining: String) -> Element {
    let tick = use_second_tick();
    let _ = tick();
    let countdown = countdown_display(&remaining, estimated_now());
    rsx! {
        span { class: countdown_class(&countdown), "{countdown.label}" }
    }
}

#[component]
pub(crate) fn LiveCatalogRow(
    row: CatalogRow,
    show_actions: bool,
    can_bid: bool,
    allow_slow: bool,
    session_ready: bool,
    authenticated: bool,
    viewer_player_id: Option<String>,
) -> Element {
    let status_label = lot_status_label(&row.status);
    let status_class = lot_status_class(&row.status);
    let is_leading = viewer_player_id
        .as_deref()
        .zip(row.leader_name.as_deref())
        .is_some_and(|(viewer, leader)| viewer == leader);
    let leading = if is_leading { "public-auction-catalog__leading" } else { "" };
    let leading_status_id = format!("public-auction-leading-status-{}", row.lot_id);
    // 非 live 行保持原占位倒计时文案与样式；live 行的倒计时下沉到 LiveRowCountdown 叶子组件。
    let inactive_countdown = (row.status != "live").then(|| CountdownDisplay {
        label: t!("value-empty"),
        urgent: false,
        expired: false,
        empty: true,
    });
    let level = row.level.map(|value| value.to_string()).unwrap_or_else(|| t!("value-empty"));
    let primary_attribute_initials =
        row.primary_attribute_initials.clone().unwrap_or_else(|| t!("value-empty"));
    let avg_roll = row.avg_roll.map(|value| format!("{value:.1}%"));
    let leader = row.leader_name.clone().unwrap_or_else(|| t!("value-empty"));
    let seller = row.seller_name.clone();
    let current_price = format_amount(row.current_price);
    let current_price_full =
        t!("price-exact", amount: format_credit_amount_full(row.current_price));
    let (next_bid, next_bid_full) = if row.next_bid > MAX_PUBLIC_BID_AMOUNT {
        (t!("bid-limit-reached"), t!("bid-limit-note"))
    } else {
        (
            format_amount(row.next_bid),
            t!("price-exact", amount: format_credit_amount_full(row.next_bid)),
        )
    };

    rsx! {
        tr {
            class: "{leading}",
            aria_describedby: if is_leading { leading_status_id.clone() } else { String::new() },
            td { "data-label": t!("catalog-code"),
                Link {
                    class: "public-auction-catalog__code",
                    to: lot_detail_href(&row.lot_id),
                    "{row.code}"
                }
            }
            td { class: "public-auction-catalog__name", "data-label": t!("catalog-item"),
                CatalogEquipmentName {
                    name: row.name.clone(),
                    game_equipment_url: row.game_equipment_url.clone(),
                }
                if is_leading {
                    span {
                        id: leading_status_id.clone(),
                        class: "public-visually-hidden",
                        {t!("bid-leading")}
                    }
                }
            }
            td {
                class: "public-auction-catalog__avg-roll public-auction-catalog__number",
                "data-label": t!("equipment-avg-roll"),
                if let Some(avg_roll) = avg_roll {
                    "{avg_roll}"
                } else {
                    span { class: "public-auction-catalog__none", {t!("value-empty")} }
                }
            }
            td { "data-label": t!("catalog-level"), "{level}" }
            td {
                class: "public-auction-catalog__primary-attributes",
                "data-label": t!("catalog-primary-attributes"),
                "{primary_attribute_initials}"
            }
            td { "data-label": t!("catalog-seller"),
                TruncatedText {
                    value: seller,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-current"),
                title: current_price_full.clone(),
                aria_label: current_price_full,
                "{current_price}"
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-next-bid"),
                title: next_bid_full.clone(),
                aria_label: next_bid_full,
                "{next_bid}"
            }
            td { class: "public-auction-catalog__participant-cell", "data-label": t!("catalog-leader"),
                if row.leader_name.is_some() {
                    CatalogParticipant {
                        name: leader,
                        marker: None,
                        trailing: None,
                    }
                } else {
                    span { class: "public-auction-catalog__none", "{leader}" }
                }
            }
            td {
                "data-label": t!("catalog-remaining"),
                if row.status == "live" {
                    LiveRowCountdown { remaining: row.remaining.clone() }
                } else if let Some(countdown) = inactive_countdown {
                    span { class: countdown_class(&countdown), "{countdown.label}" }
                }
            }
            td { "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status_class}", "{status_label}" }
            }
            if show_actions {
                td { class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                    // 竞价期内已结束的拍品保留空单元格，与仍可竞价的同表拍品对齐。
                    if row.status == "live" {
                        div { class: "public-auction-catalog__action-controls",
                            LiveCatalogActions {
                                lot_id: row.lot_id.clone(),
                                lot_code: row.code.clone(),
                                can_bid,
                                current_price: row.current_price,
                                next_bid: row.next_bid,
                                lot_status: row.status.clone(),
                                allow_slow,
                                session_ready,
                                block_ordinary_for_active_proxy: false,
                                authenticated,
                                leading: is_leading,
                                show_view: false,
                                show_proxy_summary: false,
                            }
                            LotWatchButton { lot_id: row.lot_id.clone(), lot_code: row.code.clone() }
                        }
                    }
                }
            }
        }
    }
}
