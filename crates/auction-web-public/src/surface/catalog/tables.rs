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
    active_equipment_group: String,
    level_filtered: bool,
    page_number: i64,
    on_category_change: EventHandler<String>,
    on_page_change: EventHandler<i64>,
    on_page_clamp: EventHandler<i64>,
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
    let columns_open = use_signal(|| false);
    let visibility = use_signal(CatalogVisibility::load);
    let active = active_category;
    let indices: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| active == "all" || catalog_category_key(&row.category) == active)
        .map(|(index, _)| index)
        .collect();
    let is_live = projection_kind == "live";
    // 历史成交目录仍使用 live 价格投影；是否保留操作列单独按拍卖阶段决定。
    let show_actions = auction_status != "settled";
    let layout = if !is_live {
        CatalogLayout::Preview
    } else if show_actions {
        CatalogLayout::Live
    } else {
        CatalogLayout::Settled
    };
    let effective_visibility = visibility().for_layout(layout);
    let counts = std::array::from_fn(|index| {
        let category = if index == 0 { "all" } else { CATALOG_CATEGORIES[index - 1].0 };
        catalog_count(&category_counts, category, &rows)
    });

    rsx! {
        div { class: "public-auction-type-toolbar",
            presentation::desktop::CategoryFilter {
                active: active.clone(),
                counts,
                on_change: on_category_change,
            }
            presentation::mobile::CategoryFilter {
                active: active.clone(),
                counts,
                on_change: on_category_change,
            }
            CatalogColumnsButton { open: columns_open }
        }
        CatalogColumnsPanel { open: columns_open, visibility, layout }
        if indices.is_empty() {
            div { class: "ui-async-state", role: "status",
                p { class: "ui-async-state__title",
                    if active_equipment_group != "all" || level_filtered {
                        {t!("catalog-empty-filtered")}
                    } else if active == "all" {
                        {t!("catalog-empty-all")}
                    } else {
                        {t!("catalog-empty-category")}
                    }
                }
            }
        } else if is_live {
            LiveCatalogTable {
                rows: Rc::clone(&rows),
                indices,
                visibility: effective_visibility,
                show_actions,
                can_bid,
                allow_slow,
                session_ready,
                authenticated,
                viewer_player_id: viewer_player_id.clone(),
            }
        } else {
            PreviewCatalogTable {
                rows: Rc::clone(&rows),
                indices,
                visibility: effective_visibility,
            }
        }
        PublicPager {
            page: page_number,
            page_size,
            total: page_total,
            has_more,
            label: t!("auction-pager-label"),
            on_page_change,
            on_page_clamp,
            on_page_size_change,
        }
    }
}
#[component]
pub(crate) fn LiveCatalogTable(
    rows: Rc<Vec<CatalogRow>>,
    indices: Vec<usize>,
    visibility: CatalogVisibility,
    show_actions: bool,
    can_bid: bool,
    allow_slow: bool,
    session_ready: bool,
    authenticated: bool,
    viewer_player_id: Option<String>,
) -> Element {
    // colgroup 分配完整百分比宽度；窄屏由 CSS 将同一行列 DOM 展示为字段卡片。
    let (layout, column_layout) = if show_actions {
        ("public-catalog-live", CatalogLayout::Live)
    } else {
        ("public-catalog-settled", CatalogLayout::Settled)
    };
    let widths = visibility.widths(column_layout);
    rsx! {
        div { class: "public-auction-catalog",
            table { class: "public-auction-catalog__table public-auction-catalog__table--live",
                caption { class: "public-visually-hidden", {t!("auction-lots")} }
                auction_web_shared::TableColumns { layout, widths }
                thead {
                    tr {
                        th { hidden: !visibility.contains(CatalogColumn::Code), scope: "col", {t!("catalog-code")} }
                        th { hidden: !visibility.contains(CatalogColumn::Item), scope: "col", {t!("catalog-item")} }
                        th { hidden: !visibility.contains(CatalogColumn::Group), scope: "col", {t!("catalog-group")} }
                        th { hidden: !visibility.contains(CatalogColumn::AverageRoll), scope: "col", class: "public-auction-catalog__number", {t!("equipment-avg-roll")} }
                        th { hidden: !visibility.contains(CatalogColumn::Level), scope: "col", {t!("catalog-level")} }
                        th { hidden: !visibility.contains(CatalogColumn::Attributes), scope: "col", {t!("catalog-primary-attributes")} }
                        th { hidden: !visibility.contains(CatalogColumn::Seller), scope: "col", {t!("catalog-seller")} }
                        th { hidden: !visibility.contains(CatalogColumn::Price), scope: "col", class: "public-auction-catalog__number", {t!("catalog-current")} }
                        th { hidden: !visibility.contains(CatalogColumn::NextBid), scope: "col", class: "public-auction-catalog__number", {t!("catalog-next-bid")} }
                        th { hidden: !visibility.contains(CatalogColumn::Leader), scope: "col", {t!("catalog-leader")} }
                        th { hidden: !visibility.contains(CatalogColumn::Remaining), scope: "col", {t!("catalog-remaining")} }
                        th { hidden: !visibility.contains(CatalogColumn::Status), scope: "col", {t!("catalog-status")} }
                        if show_actions {
                            th { hidden: !visibility.contains(CatalogColumn::Actions), scope: "col", {t!("catalog-actions")} }
                        }
                    }
                }
                tbody {
                    for index in indices {
                        LiveCatalogRow {
                            // 以 lot_id 作为稳定 key，避免刷新后行状态错位绑定到别的 lot。
                            key: "{rows[index].lot_id}",
                            row: rows[index].clone(),
                            visibility,
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
pub(crate) fn PreviewCatalogTable(
    rows: Rc<Vec<CatalogRow>>,
    indices: Vec<usize>,
    visibility: CatalogVisibility,
) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table { class: "public-auction-catalog__table public-auction-catalog__table--preview",
                caption { class: "public-visually-hidden", {t!("auction-preview-lots")} }
                // 预览目录不提供行内操作；默认十一列，隐藏列后按相同权重重新归一化。
                auction_web_shared::TableColumns { layout: "public-catalog-preview", widths: visibility.widths(CatalogLayout::Preview) }
                thead {
                    tr {
                        th { hidden: !visibility.contains(CatalogColumn::Code), scope: "col", {t!("catalog-code")} }
                        th { hidden: !visibility.contains(CatalogColumn::Item), scope: "col", {t!("catalog-item")} }
                        th { hidden: !visibility.contains(CatalogColumn::Group), scope: "col", {t!("catalog-group")} }
                        th { hidden: !visibility.contains(CatalogColumn::AverageRoll), scope: "col", class: "public-auction-catalog__number", {t!("equipment-avg-roll")} }
                        th { hidden: !visibility.contains(CatalogColumn::Level), scope: "col", {t!("catalog-level")} }
                        th { hidden: !visibility.contains(CatalogColumn::Attributes), scope: "col", {t!("catalog-primary-attributes")} }
                        th { hidden: !visibility.contains(CatalogColumn::Seller), scope: "col", {t!("catalog-seller")} }
                        th { hidden: !visibility.contains(CatalogColumn::Price), scope: "col", class: "public-auction-catalog__number", {t!("catalog-start-price")} }
                        th { hidden: !visibility.contains(CatalogColumn::Leader), scope: "col", {t!("catalog-leader")} }
                        th { hidden: !visibility.contains(CatalogColumn::Remaining), scope: "col", {t!("catalog-remaining")} }
                        th { hidden: !visibility.contains(CatalogColumn::Status), scope: "col", {t!("catalog-status")} }
                    }
                }
                tbody {
                    for index in indices {
                        // 以 lot_id 作为稳定 key，避免刷新后行状态错位绑定到别的 lot。
                        PreviewCatalogRow {
                            key: "{rows[index].lot_id}",
                            row: rows[index].clone(),
                            visibility,
                        }
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
    let countdown = use_countdown_display(remaining);
    rsx! {
        span { class: countdown_class(&countdown), "{countdown.label}" }
    }
}

#[component]
pub(crate) fn LiveCatalogRow(
    row: CatalogRow,
    visibility: CatalogVisibility,
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
    let show_remaining = visibility.contains(CatalogColumn::Remaining);
    // 非 live 行保持原占位倒计时文案与样式；live 行的倒计时下沉到 LiveRowCountdown 叶子组件。
    let inactive_countdown = (show_remaining && row.status != "live").then(|| CountdownDisplay {
        label: t!("value-empty"),
        urgent: false,
        expired: false,
        empty: true,
    });
    let leader = row.leader_name.clone().unwrap_or_else(|| t!("value-empty"));
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
            td { hidden: !visibility.contains(CatalogColumn::Code), "data-label": t!("catalog-code"),
                Link {
                    class: "public-auction-catalog__code",
                    to: lot_detail_href(&row.lot_id),
                    "{row.code}"
                }
            }
            td { hidden: !visibility.contains(CatalogColumn::Item), class: "public-auction-catalog__name", "data-label": t!("catalog-item"),
                CatalogEquipmentName {
                    name: row.name.clone(),
                    game_equipment_url: row.game_equipment_url.clone(),
                }
                // 显式描述引用保留在 DOM；装备列隐藏时，领先状态仍可由行的 aria-describedby 读取。
                if is_leading {
                    span {
                        id: leading_status_id.clone(),
                        class: "public-visually-hidden",
                        {t!("bid-leading")}
                    }
                }
            }
            CatalogEquipmentCells { row: row.clone(), visibility }
            td {
                hidden: !visibility.contains(CatalogColumn::Price),
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-current"),
                title: current_price_full.clone(),
                aria_label: current_price_full,
                "{current_price}"
            }
            td {
                hidden: !visibility.contains(CatalogColumn::NextBid),
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-next-bid"),
                title: next_bid_full.clone(),
                aria_label: next_bid_full,
                "{next_bid}"
            }
            td { hidden: !visibility.contains(CatalogColumn::Leader), class: "public-auction-catalog__participant-cell", "data-label": t!("catalog-leader"),
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
                hidden: !show_remaining,
                class: "public-auction-catalog__remaining",
                "data-label": t!("catalog-remaining"),
                // 隐藏时卸载倒计时叶子组件，其订阅守卫随之释放；整行和命令宿主不受影响。
                if show_remaining && row.status == "live" {
                    LiveRowCountdown { remaining: row.remaining.clone() }
                } else if let Some(countdown) = inactive_countdown {
                    span { class: countdown_class(&countdown), "{countdown.label}" }
                }
            }
            td {
                hidden: !visibility.contains(CatalogColumn::Status),
                class: "public-auction-catalog__status",
                "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status_class}", "{status_label}" }
            }
            if show_actions {
                td { hidden: !visibility.contains(CatalogColumn::Actions), class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                    // 列偏好只隐藏单元格，不卸载命令宿主，保留冻结的 command_id 与 unknown 状态。
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
                                show_view: true,
                                show_proxy_summary: false,
                            }
                            LotWatchButton {
                                lot_id: row.lot_id.clone(),
                                lot_code: row.code.clone(),
                                icon_only: true,
                            }
                        }
                    }
                }
            }
        }
    }
}
