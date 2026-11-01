//! Public 补偿池各世界余额、批次表格和展开行。

use super::*;

#[component]
pub(super) fn CompensationPoolTable(
    page: CompensationPoolPage,
    selected_auction_id: String,
    lots: Option<Result<Option<CompensationLotPage>, String>>,
    on_page_change: EventHandler<i64>,
    on_page_clamp: EventHandler<i64>,
    on_page_size_change: EventHandler<i64>,
    on_select_round: EventHandler<String>,
    on_lot_page_change: EventHandler<i64>,
    on_lot_page_clamp: EventHandler<i64>,
    on_lot_page_size_change: EventHandler<i64>,
    on_retry_lots: EventHandler<()>,
) -> Element {
    let page_size = page.page_size.max(1);
    rsx! {
        section { class: "public-compensation-page", aria_labelledby: "compensation-pool-title",
            header { class: "public-compensation-heading",
                div {
                    h1 { id: "compensation-pool-title", {t!("page-compensation-pool-title")} }
                    p { class: "public-compensation-intro", {t!("page-compensation-pool-note")} }
                    p { class: "public-compensation-intro", {t!("compensation-formula")} }
                }
                // 各世界补偿池独立结算，逐个展示余额，不再合计成一个数字。
                div { class: "public-compensation-pools", aria_label: t!("compensation-balance"),
                    for pool in page.pools.iter() {
                        div { key: "{pool.world_id}", class: "public-compensation-balance",
                            span { {t!("compensation-world-balance", world: world_label(&pool.world_id))} }
                            strong { "{format_amount(pool.balance)}" }
                            small {
                                time {
                                    datetime: pool.updated_at.clone(),
                                    {t!("compensation-updated", date: format_auction_time(&pool.updated_at))}
                                }
                            }
                        }
                    }
                }
            }
            if page.rounds.is_empty() {
                p { class: "public-compensation-empty", {t!("compensation-empty")} }
            } else {
                div { class: "public-compensation-table-wrap ui-table--fit",
                    table { class: "public-compensation-table",
                        auction_web_shared::TableColumns { layout: "public-compensation", widths: vec![4, 6, 7, 10, 10, 10, 10, 7, 10, 9, 17] }
                        thead {
                            tr {
                                th { scope: "col", class: "public-compensation-toggle-cell", aria_label: t!("compensation-details") }
                                th { scope: "col", {t!("compensation-round-label")} }
                                th { scope: "col", {t!("compensation-world")} }
                                th { scope: "col", class: "public-compensation-number", {t!("compensation-opening")} }
                                th { scope: "col", class: "public-compensation-number", {t!("compensation-tax")} }
                                th { scope: "col", class: "public-compensation-number", {t!("compensation-discount")} }
                                th { scope: "col", class: "public-compensation-number", {t!("compensation-out")} }
                                th { scope: "col", class: "public-compensation-number", {t!("compensation-coverage")} }
                                th { scope: "col", class: "public-compensation-number", {t!("compensation-closing")} }
                                th { scope: "col", class: "public-compensation-number", {t!("compensation-reversal")} }
                                th { scope: "col", {t!("compensation-settled")} }
                            }
                        }
                        tbody {
                            for round in page.rounds {
                                {
                                    let open = selected_auction_id == round.auction_id;
                                    // 只有展开行消费明细；不能给每个折叠批次复制完整明细页。
                                    let lots = if open { lots.clone() } else { None };
                                    rsx! {
                                        CompensationRoundRow {
                                            round,
                                            open,
                                            lots,
                                            on_select_round,
                                            on_lot_page_change,
                                            on_lot_page_clamp,
                                            on_lot_page_size_change,
                                            on_retry_lots,
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            PublicPager {
                page: page.page.unwrap_or(1),
                page_size,
                total: page.total,
                has_more: page.has_more.unwrap_or(false),
                label: t!("page-compensation-pool-title"),
                on_page_change,
                on_page_clamp,
                on_page_size_change,
            }
        }
    }
}

#[component]
fn CompensationRoundRow(
    round: CompensationRound,
    open: bool,
    lots: Option<Result<Option<CompensationLotPage>, String>>,
    on_select_round: EventHandler<String>,
    on_lot_page_change: EventHandler<i64>,
    on_lot_page_clamp: EventHandler<i64>,
    on_lot_page_size_change: EventHandler<i64>,
    on_retry_lots: EventHandler<()>,
) -> Element {
    let details_id = format!("compensation-details-{}", round.auction_id);
    let auction_id = round.auction_id.clone();
    let reversal = if round.reversal_count > 0 {
        t!(
            "compensation-reversal-summary",
            amount: signed_amount(round.reversal_amount),
            count: round.reversal_count
        )
    } else {
        "—".to_owned()
    };
    rsx! {
        tr { class: "public-compensation-round",
            td { class: "public-compensation-toggle-cell", "data-label": t!("compensation-details"),
                button {
                    class: "public-history-group-toggle public-compensation-expand",
                    r#type: "button",
                    aria_controls: details_id.clone(),
                    aria_expanded: open.to_string(),
                    aria_label: if open {
                        t!("compensation-collapse", sequence: round.auction_sequence)
                    } else {
                        t!("compensation-expand", sequence: round.auction_sequence)
                    },
                    onclick: move |_| on_select_round.call(auction_id.clone()),
                    span { aria_hidden: "true", if open { "−" } else { "+" } }
                }
            }
            td { "data-label": t!("compensation-round-label"), "#{round.auction_sequence}" }
            td { "data-label": t!("compensation-world"), "{world_label(&round.world_id)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-opening"), "{format_amount(round.opening_balance)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-tax"), "{format_amount(round.tax_income)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-discount"), "{format_amount(round.discount_demand)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-out"), "{format_amount(round.compensation_out)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-coverage"),
                "{coverage_label(round.compensation_out, round.discount_demand)}"
            }
            td { class: "public-compensation-number public-compensation-closing", "data-label": t!("compensation-closing"), "{format_amount(round.closing_balance)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-reversal"), "{reversal}" }
            td { "data-label": t!("compensation-settled"),
                Time { value: round.settled_at.clone() }
            }
        }
        if open {
            tr { id: details_id.clone(), class: "public-compensation-details-row",
                td { colspan: 11,
                    CompensationLotDetails {
                        round: round.clone(),
                        lots,
                        on_lot_page_change,
                        on_lot_page_clamp,
                        on_lot_page_size_change,
                        on_retry_lots,
                    }
                }
            }
        }
    }
}
