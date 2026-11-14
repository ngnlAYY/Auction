//! 价格历史的分组表格。

use super::presentation::history_groups;
use super::presentation::history_median_amount;
use crate::api::{PriceHistoryEntry, PriceHistoryPage, PriceHistoryStats, lot_code_for};
use crate::surface::{
    PublicPager, Time, catalog_category_key, catalog_category_label, format_credit_amount,
    world_label,
};
use auction_web_shared::TruncatedText;
use auction_web_shared::t;
use dioxus::prelude::*;

use crate::surface::LotDetailLookupButton;

#[component]
pub(crate) fn PriceHistoryTable(
    page: PriceHistoryPage,
    grouping: String,
    current_page: i64,
    on_buyer_filter: EventHandler<String>,
    on_seller_filter: EventHandler<String>,
    on_page_change: EventHandler<i64>,
    on_page_clamp: EventHandler<i64>,
    on_page_size_change: EventHandler<i64>,
) -> Element {
    let page_size = page.page_size.max(1);
    let empty = page.rows.is_empty();
    // 当前组件拥有本页快照，分组直接接管行，不为派生展示复制整页 DTO。
    let groups = history_groups(page.rows, &grouping);
    rsx! {
        div { class: "public-history-results",
            if let Some(stats) = page.stats.filter(|stats| stats.count > 0) {
                PriceHistoryStatsView { stats }
            }
            if empty {
                div { class: "public-auction-empty", role: "status",
                    p { class: "public-auction-empty__title", {t!("history-empty-title")} }
                    p { class: "public-auction-empty__hint", {t!("history-empty-hint")} }
                }
            } else {
                div { class: "public-history-result-groups",
                    for group in groups {
                        if grouping == "flat" {
                            PriceHistoryRowsTable {
                                rows: group.rows,
                                title: t!("page-history-title"),
                                on_buyer_filter,
                                on_seller_filter,
                            }
                        } else {
                            PriceHistoryGroupPanel {
                                key: "{grouping}:{current_page}:{page_size}:{group.key}",
                                label: group.label,
                                rows: group.rows,
                                on_buyer_filter,
                                on_seller_filter,
                            }
                        }
                    }
                }
            }
            PublicPager {
                page: current_page,
                page_size,
                total: page.total,
                has_more: page.has_more.unwrap_or(false),
                label: t!("page-history-title"),
                on_page_change,
                on_page_clamp,
                on_page_size_change,
            }
        }
    }
}

/// 每组独立呈现结果框；组头始终保留，折叠只卸载本组表格，不读取其他分页。
#[component]
fn PriceHistoryGroupPanel(
    label: String,
    rows: Vec<PriceHistoryEntry>,
    on_buyer_filter: EventHandler<String>,
    on_seller_filter: EventHandler<String>,
) -> Element {
    let mut expanded = use_signal(|| true);
    let count = rows.len();
    rsx! {
        section { class: "public-history-group-panel", aria_label: label,
            h2 { class: "public-history-group-heading",
                button { r#type: "button", class: "public-history-group-toggle",
                    aria_expanded: expanded().to_string(),
                    onclick: move |_| expanded.set(!expanded()),
                    span { class: "public-chevron", aria_hidden: "true" }
                    TruncatedText { value: label.clone(), size: "wide", focusable: false, portal: true }
                    span { class: "public-history-group-count", {t!("history-group-count", count: count)} }
                }
            }
            if expanded() {
                PriceHistoryRowsTable { rows, title: label.clone(), on_buyer_filter, on_seller_filter }
            }
        }
    }
}

/// 各结果框复用相同十列表头与列宽，标题关联当前组；窄屏仍保留原生表格结构。
#[component]
fn PriceHistoryRowsTable(
    rows: Vec<PriceHistoryEntry>,
    title: String,
    on_buyer_filter: EventHandler<String>,
    on_seller_filter: EventHandler<String>,
) -> Element {
    rsx! {
        div { class: "public-history-table-wrap",
            table { class: "public-history-table",
                aria_label: "{title}",
                auction_web_shared::TableColumns { layout: "public-price-history", widths: vec![22, 9, 5, 7, 6, 11, 9, 9, 16, 6] }
                thead {
                    tr {
                        th { scope: "col", {t!("history-item")} }
                        th { scope: "col", {t!("history-category")} }
                        th { scope: "col", class: "public-history-number", {t!("history-level")} }
                        th { scope: "col", {t!("history-world")} }
                        th { scope: "col", {t!("history-round")} }
                        th { scope: "col", class: "public-history-number", {t!("history-price")} }
                        th { scope: "col", {t!("history-buyer")} }
                        th { scope: "col", {t!("history-seller")} }
                        th { scope: "col", {t!("history-closed")} }
                        th { scope: "col", class: "public-lot-view-cell", {t!("catalog-actions")} }
                    }
                }
                tbody {
                    for row in rows {
                        PriceHistoryRow { key: "{row.lot_id}", row, on_buyer_filter, on_seller_filter }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn PriceHistoryStatsView(stats: PriceHistoryStats) -> Element {
    let median = history_median_amount(&stats);
    rsx! {
        dl { class: "public-history-stats", aria_label: t!("history-statistics"),
            div { class: "public-history-stat",
                dt { {t!("history-count")} }
                dd { "{stats.count}" }
            }
            div { class: "public-history-stat",
                dt { {t!("history-min")} }
                dd { "{format_credit_amount(stats.min)}" }
            }
            div { class: "public-history-stat",
                dt { {t!("history-max")} }
                dd { "{format_credit_amount(stats.max)}" }
            }
            div { class: "public-history-stat",
                dt { {t!("history-median")} }
                dd { "{median}" }
            }
        }
    }
}

#[component]
pub(crate) fn PriceHistoryRow(
    row: PriceHistoryEntry,
    on_buyer_filter: EventHandler<String>,
    on_seller_filter: EventHandler<String>,
) -> Element {
    let auction_href = history_auction_href(&row.auction_id, &row.world_id);
    let lot_code = lot_code_for(&row.equipment_category, row.lot_number);
    let equipment_label = format!("{lot_code} · {}", row.equipment_name);
    let level = row.equipment_level.map_or_else(|| t!("value-empty"), |level| level.to_string());
    let world = world_label(&row.world_id);
    let category = catalog_category_label(catalog_category_key(&row.equipment_category));
    let price = format_credit_amount(row.price);
    let buyer = row.buyer_name.clone();
    let seller = row.seller_name.clone();
    rsx! {
        tr {
            // 名称不是链接：详情统一经操作栏的查看按钮在页内抽屉打开。名称外没有链接承接
            // 焦点，截断时由文本自身进入 Tab 顺序以查看完整名称。
            td { class: "public-history-equipment", "data-label": t!("history-item"),
                TruncatedText { value: equipment_label, size: "wide", portal: true }
            }
            td { "data-label": t!("history-category"), "{category}" }
            td { class: "public-history-number", "data-label": t!("history-level"), "{level}" }
            td { "data-label": t!("history-world"), "{world}" }
            td { class: "public-history-auction", "data-label": t!("history-round"),
                Link { to: auction_href, "#{row.auction_sequence}" }
            }
            td {
                class: "public-history-price public-history-number",
                "data-label": t!("history-price"),
                "{price}"
            }
            td { "data-label": t!("history-buyer"),
                button { r#type: "button", class: "public-history-player-filter",
                    aria_label: t!("history-filter-by-buyer", name: buyer.clone()),
                    title: t!("history-filter-by-buyer", name: buyer.clone()),
                    onclick: move |_| on_buyer_filter.call(buyer.clone()),
                    TruncatedText { value: row.buyer_name, size: "compact", focusable: false, portal: true }
                }
            }
            td { "data-label": t!("history-seller"),
                if seller == "—" {
                    TruncatedText { value: row.seller_name, size: "compact", portal: true }
                } else {
                    button { r#type: "button", class: "public-history-player-filter",
                        aria_label: t!("history-filter-by-seller", name: seller.clone()),
                        title: t!("history-filter-by-seller", name: seller.clone()),
                        onclick: move |_| on_seller_filter.call(seller.clone()),
                        TruncatedText { value: row.seller_name, size: "compact", focusable: false, portal: true }
                    }
                }
            }
            td { "data-label": t!("history-closed"),
                Time { value: row.closed_at.clone() }
            }
            td { class: "public-lot-view-cell", "data-label": t!("catalog-actions"),
                LotDetailLookupButton {
                    auction_id: row.auction_id.clone(),
                    lot_id: row.lot_id.clone(),
                    lot_code,
                    world: Some(row.world_id),
                }
            }
        }
    }
}

pub(crate) fn history_auction_href(auction_id: &str, world_id: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("auction", auction_id);
    if world_id != "main" {
        query.append_pair("world", world_id);
    }
    format!("/?{}", query.finish())
}
