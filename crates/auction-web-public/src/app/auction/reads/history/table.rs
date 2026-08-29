use super::*;

#[component]
pub(crate) fn PriceHistoryTable(
    page: PriceHistoryPage,
    current_page: i64,
    on_page_change: EventHandler<i64>,
) -> Element {
    let page_size = page.page_size.max(1);
    let rows = page.rows.clone();
    rsx! {
        div { class: "public-history-results",
            if let Some(stats) = page.stats.clone().filter(|stats| stats.count > 0) {
                PriceHistoryStatsView { stats }
            }
            if rows.is_empty() {
                div { class: "public-auction-empty", role: "status",
                    p { class: "public-auction-empty__title", {t!("history-empty-title")} }
                    p { class: "public-auction-empty__hint", {t!("history-empty-hint")} }
                }
            } else {
                div { class: "public-history-table-wrap",
                    table {
                        class: "public-history-table",
                        thead {
                            tr {
                                th { {t!("history-item")} }
                                th { {t!("history-category")} }
                                th { class: "public-history-number", {t!("history-level")} }
                                th { {t!("history-world")} }
                                th { {t!("history-round")} }
                                th { class: "public-history-number", {t!("history-price")} }
                                th { {t!("history-buyer")} }
                                th { {t!("history-seller")} }
                                th { {t!("history-closed")} }
                            }
                        }
                        tbody {
                            for row in rows {
                                PriceHistoryRow { row }
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
            }
        }
    }
}

#[component]
pub(crate) fn PriceHistoryStatsView(stats: PriceHistoryStats) -> Element {
    rsx! {
        dl { class: "public-history-stats", aria_label: t!("history-statistics"),
            div { class: "public-history-stat",
                dt { {t!("history-count")} }
                dd { "{stats.count}" }
            }
            div { class: "public-history-stat",
                dt { {t!("history-min")} }
                dd { "{super::format_amount(stats.min)}" }
            }
            div { class: "public-history-stat",
                dt { {t!("history-max")} }
                dd { "{super::format_amount(stats.max)}" }
            }
            div { class: "public-history-stat",
                dt { {t!("history-median")} }
                dd { "{super::format_amount(stats.median)}" }
            }
        }
    }
}

#[component]
pub(crate) fn PriceHistoryRow(row: PriceHistoryEntry) -> Element {
    let lot_href = format!("/lots/{}", row.lot_id);
    let auction_href = history_auction_href(&row.auction_id, &row.world_id);
    let level =
        row.equipment_level.map(|level| level.to_string()).unwrap_or_else(|| t!("value-empty"));
    let world = super::world_label(&row.world_id);
    let category =
        super::catalog_category_label(super::catalog_category_key(&row.equipment_category));
    let price = super::format_amount(row.price);
    rsx! {
        tr {
            td { class: "public-history-equipment", "data-label": t!("history-item"),
                Link { to: lot_href, title: row.equipment_name.clone(), "{row.equipment_name}" }
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
            td { "data-label": t!("history-buyer"), "{row.buyer_name}" }
            td { "data-label": t!("history-seller"), "{row.seller_name}" }
            td { "data-label": t!("history-closed"),
                time { datetime: row.closed_at.clone(), "{super::format_auction_time(&row.closed_at)}" }
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
