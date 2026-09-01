//! Public 拍卖目录筛选、分组表格和分页。

use super::*;

#[component]
pub(crate) fn CatalogTable(
    rows: Vec<CatalogRow>,
    now: DateTime<Utc>,
    projection_kind: String,
    category_counts: BTreeMap<String, i64>,
    mut active_category: Signal<String>,
    mut page_number: Signal<i64>,
    page_total: Option<i64>,
    page_size: i64,
    has_more: bool,
    can_bid: bool,
    allow_slow: bool,
    session_ready: bool,
    authenticated: bool,
    viewer_name: Option<String>,
) -> Element {
    let active = active_category();
    let groups = grouped_catalog_rows(&rows, &active);
    let is_live = projection_kind == "live";
    let all_count = catalog_count(&category_counts, "all", &rows);

    rsx! {
        nav { class: "public-auction-filter", aria_label: t!("filter-label"),
            button {
                class: "public-auction-filter__tab",
                r#type: "button",
                aria_pressed: active == "all",
                onclick: move |_| {
                    active_category.set("all".to_owned());
                    page_number.set(1);
                    update_public_query(
                        |query| {
                            query.category = None;
                            query.page = 1;
                        },
                        false,
                    );
                },
                {t!("filter-all")}
                span { class: "public-auction-filter__count", "{all_count}" }
            }
            for (category, _) in CATALOG_CATEGORIES {
                button {
                    class: "public-auction-filter__tab",
                    r#type: "button",
                    aria_pressed: active == category,
                    onclick: move |_| {
                        active_category.set(category.to_owned());
                        page_number.set(1);
                        update_public_query(
                            |query| {
                                query.category = Some(category.to_owned());
                                query.page = 1;
                            },
                            false,
                        );
                    },
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
        for (category, category_rows) in groups {
            section { class: "public-auction-category",
                h3 { class: "public-auction-category__title",
                    {catalog_category_label(category)}
                    span { class: "public-auction-category__code", "{catalog_category_code(category)}" }
                }
                if is_live {
                    LiveCatalogTable {
                        rows: category_rows,
                        now,
                        can_bid,
                        allow_slow,
                        session_ready,
                        authenticated,
                        viewer_name: viewer_name.clone(),
                    }
                } else {
                    PreviewCatalogTable { rows: category_rows }
                }
            }
        }
        PublicPager {
            page: page_number(),
            page_size,
            total: page_total,
            has_more,
            label: t!("auction-pager-label"),
            on_page_change: move |page| {
                page_number.set(page);
                update_public_query(|query| query.page = page, false);
            },
        }
    }
}
#[component]
pub(crate) fn LiveCatalogTable(
    rows: Vec<CatalogRow>,
    now: DateTime<Utc>,
    can_bid: bool,
    allow_slow: bool,
    session_ready: bool,
    authenticated: bool,
    viewer_name: Option<String>,
) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table {
                caption { class: "public-visually-hidden", {t!("auction-lots")} }
                thead {
                    tr {
                        th { scope: "col", {t!("catalog-code")} }
                        th { scope: "col", {t!("catalog-item")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("equipment-avg-roll")} }
                        th { scope: "col", {t!("catalog-level")} }
                        th { scope: "col", {t!("catalog-seller")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("catalog-current")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("catalog-next-bid")} }
                        th { scope: "col", {t!("catalog-leader")} }
                        th { scope: "col", {t!("catalog-remaining")} }
                        th { scope: "col", {t!("catalog-status")} }
                        th { scope: "col", span { class: "public-visually-hidden", {t!("catalog-actions")} } }
                    }
                }
                tbody {
                    for row in rows {
                        LiveCatalogRow {
                            row,
                            now,
                            can_bid,
                            allow_slow,
                            session_ready,
                            authenticated,
                            viewer_name: viewer_name.clone(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn PreviewCatalogTable(rows: Vec<CatalogRow>) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table {
                caption { class: "public-visually-hidden", {t!("auction-preview-lots")} }
                thead {
                    tr {
                        th { scope: "col", {t!("catalog-code")} }
                        th { scope: "col", {t!("catalog-item")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("equipment-avg-roll")} }
                        th { scope: "col", {t!("catalog-level")} }
                        th { scope: "col", {t!("catalog-seller")} }
                        th { scope: "col", class: "public-auction-catalog__number", {t!("catalog-start-price")} }
                        th { scope: "col", {t!("catalog-leader")} }
                        th { scope: "col", {t!("catalog-remaining")} }
                        th { scope: "col", {t!("catalog-status")} }
                        th { scope: "col", span { class: "public-visually-hidden", {t!("catalog-actions")} } }
                    }
                }
                tbody {
                    for row in rows {
                        PreviewCatalogRow { row }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn LiveCatalogRow(
    row: CatalogRow,
    now: DateTime<Utc>,
    can_bid: bool,
    allow_slow: bool,
    session_ready: bool,
    authenticated: bool,
    viewer_name: Option<String>,
) -> Element {
    let status_label = lot_status_label(&row.status);
    let status_class = lot_status_class(&row.status);
    let is_leading = viewer_name
        .as_deref()
        .zip(row.leader_name.as_deref())
        .is_some_and(|(viewer, leader)| viewer == leader);
    let leading = if is_leading { "public-auction-catalog__leading" } else { "" };
    let countdown = if row.status == "live" {
        countdown_display(&row.remaining, now)
    } else {
        CountdownDisplay { label: t!("value-empty"), urgent: false, expired: false, empty: true }
    };
    let countdown_css_class = countdown_class(&countdown);
    let level = row.level.map(|value| value.to_string()).unwrap_or_else(|| t!("value-empty"));
    let avg_roll = row.avg_roll.map(|value| format!("{value:.1}%"));
    let leader = row.leader_name.clone().unwrap_or_else(|| t!("value-empty"));
    let seller = row.seller_name.clone();
    let current_price = format_amount(row.current_price);
    let next_bid = format_amount(row.next_bid);

    rsx! {
        tr { class: "{leading}",
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
                "{current_price}"
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-next-bid"),
                "{next_bid}"
            }
            td { class: "public-auction-catalog__participant-cell", "data-label": t!("catalog-leader"),
                if row.leader_name.is_some() {
                    CatalogParticipant {
                        name: leader,
                        marker: None,
                        trailing: is_leading.then(|| t!("catalog-you")),
                    }
                } else {
                    span { class: "public-auction-catalog__none", "{leader}" }
                }
            }
            td {
                "data-label": t!("catalog-remaining"),
                span { class: "{countdown_css_class}", "{countdown.label}" }
            }
            td { "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status_class}", "{status_label}" }
            }
            td { class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                LiveCatalogActions {
                    lot_id: row.lot_id.clone(),
                    lot_code: row.code.clone(),
                    can_bid: can_bid && row.status == "live",
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
            }
        }
    }
}
