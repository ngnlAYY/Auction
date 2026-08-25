//! Public 补偿池批次表格和展开行。

use super::*;

#[component]
pub(super) fn CompensationPoolTable(
    page: CompensationPoolPage,
    selected_auction_id: String,
    movements: Option<Result<Option<CompensationMovementPage>, String>>,
    on_page_change: EventHandler<i64>,
    on_select_round: EventHandler<String>,
    on_movement_page_change: EventHandler<i64>,
    on_retry_movements: EventHandler<()>,
) -> Element {
    let page_size = page.page_size.max(1);
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "compensation-pool-title",
            header { class: "public-auction-category__header",
                div {
                    h1 { id: "compensation-pool-title", class: "public-auction-category__title",
                        {t!("page-compensation-pool-title")}
                    }
                    p { class: "public-auction-page__status", {t!("page-compensation-pool-note")} }
                }
                div { class: "public-auction-preview-stats", aria_label: t!("compensation-balance"),
                    span { {t!("compensation-balance", balance: format_amount(page.balance))} }
                    if let Some(updated_at) = page.updated_at.clone() {
                        time { datetime: updated_at.clone(), "{format_auction_time(&updated_at)}" }
                    }
                }
            }
            if page.rounds.is_empty() {
                p { class: "public-auction-page__status", {t!("data-empty")} }
            } else {
                div { class: "public-auction-catalog",
                    table {
                        thead {
                            tr {
                                th { aria_label: t!("compensation-details") }
                                th { {t!("compensation-round-label")} }
                                th { {t!("compensation-world")} }
                                th { {t!("compensation-opening")} }
                                th { {t!("compensation-tax")} }
                                th { {t!("compensation-discount")} }
                                th { {t!("compensation-out")} }
                                th { {t!("compensation-closing")} }
                                th { {t!("compensation-settled")} }
                            }
                        }
                        tbody {
                            for round in page.rounds.clone() {
                                CompensationRoundRow {
                                    round: round.clone(),
                                    open: selected_auction_id.as_str() == round.auction_id.as_str(),
                                    movements: movements.clone(),
                                    on_select_round,
                                    on_movement_page_change,
                                    on_retry_movements,
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
            }
        }
    }
}

#[component]
fn CompensationRoundRow(
    round: CompensationRound,
    open: bool,
    movements: Option<Result<Option<CompensationMovementPage>, String>>,
    on_select_round: EventHandler<String>,
    on_movement_page_change: EventHandler<i64>,
    on_retry_movements: EventHandler<()>,
) -> Element {
    let details_id = format!("compensation-details-{}", round.auction_id);
    let auction_id = round.auction_id.clone();
    rsx! {
        tr {
            td { class: "public-compensation-toggle-cell",
                button {
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
            td { "#{round.auction_sequence}" }
            td { "{world_label(&round.world_id)}" }
            td { class: "public-auction-catalog__amount", "{format_amount(round.opening_balance)}" }
            td { class: "public-auction-catalog__amount", "{format_amount(round.tax_income)}" }
            td { class: "public-auction-catalog__amount", "{format_amount(round.discount_demand)}" }
            td { class: "public-auction-catalog__amount", "{format_amount(round.compensation_out)}" }
            td { class: "public-auction-catalog__amount", "{format_amount(round.closing_balance)}" }
            td {
                time { datetime: round.settled_at.clone(), "{format_auction_time(&round.settled_at)}" }
            }
        }
        if open {
            tr { id: details_id.clone(), class: "public-compensation-details-row",
                td { colspan: 9,
                    CompensationMovementDetails {
                        round: round.clone(),
                        movements,
                        on_movement_page_change,
                        on_retry_movements,
                    }
                }
            }
        }
    }
}
