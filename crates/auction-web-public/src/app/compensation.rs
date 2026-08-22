//! Public 补偿池与补偿账本页面。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    CompensationMovementPage, CompensationPoolPage, CompensationRound,
    fetch_compensation_movements, fetch_compensation_pool,
};

use super::Placeholder;

pub(super) fn pool_page() -> Element {
    let pool = use_resource(fetch_compensation_pool);
    match &*pool.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-compensation-pool-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-compensation-pool-title"), detail: t!("data-error") }
        },
        Some(Ok(page)) => rsx! {
            CompensationPoolTable { page: page.clone() }
        },
    }
}

pub(super) fn movement_page() -> Element {
    let pool = use_resource(fetch_compensation_pool);
    match &*pool.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-compensation-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-compensation-title"), detail: t!("data-error") }
        },
        Some(Ok(page)) => match page.rounds.first() {
            None => rsx! {
                Placeholder { title: t!("page-compensation-title"), detail: t!("data-empty") }
            },
            Some(round) => rsx! {
                CompensationMovementPanel { auction_id: round.auction_id.clone() }
            },
        },
    }
}

#[component]
fn CompensationPoolTable(page: CompensationPoolPage) -> Element {
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "compensation-pool-title",
            h1 { id: "compensation-pool-title", class: "public-auction-category__title",
                {t!("page-compensation-pool-title")}
            }
            p { class: "public-auction-page__status",
                {t!("compensation-balance", balance: page.balance)}
            }
            if let Some(updated_at) = page.updated_at {
                p { class: "public-auction-page__status",
                    {t!("compensation-updated", updated_at: updated_at)}
                }
            }
            if page.rounds.is_empty() {
                p { class: "public-auction-page__status", {t!("data-empty")} }
            } else {
                div { class: "public-auction-catalog",
                    table {
                        thead {
                            tr {
                                th { {t!("compensation-auction")} }
                                th { {t!("compensation-opening")} }
                                th { {t!("compensation-tax")} }
                                th { {t!("compensation-out")} }
                                th { {t!("compensation-closing")} }
                                th { {t!("compensation-movements")} }
                                th { {t!("compensation-settled")} }
                            }
                        }
                        tbody {
                            for round in page.rounds {
                                CompensationRoundRow { round }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CompensationRoundRow(round: CompensationRound) -> Element {
    rsx! {
        tr {
            td { "{round.world_id} / #{round.auction_sequence}" }
            td { "{round.opening_balance}" }
            td { "{round.tax_income}" }
            td { "{round.compensation_out}" }
            td { "{round.closing_balance}" }
            td { "{round.movement_count}" }
            td { "{round.settled_at}" }
        }
    }
}

#[component]
fn CompensationMovementPanel(auction_id: String) -> Element {
    let movements = use_resource({
        let auction_id = auction_id.clone();
        move || {
            let auction_id = auction_id.clone();
            async move { fetch_compensation_movements(auction_id).await }
        }
    });
    match &*movements.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-compensation-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-compensation-title"), detail: t!("data-error") }
        },
        Some(Ok(page)) => rsx! {
            CompensationMovementTable { page: page.clone() }
        },
    }
}

#[component]
fn CompensationMovementTable(page: CompensationMovementPage) -> Element {
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "compensation-title",
            h1 { id: "compensation-title", class: "public-auction-category__title",
                {t!("page-compensation-title")}
            }
            p { class: "public-auction-page__status",
                {t!("compensation-round", world: page.round.world_id, sequence: page.round.auction_sequence)}
            }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("compensation-sequence")} }
                            th { {t!("compensation-kind")} }
                            th { {t!("compensation-lot")} }
                            th { {t!("compensation-amount")} }
                            th { {t!("compensation-balance-after")} }
                            th { {t!("compensation-created")} }
                        }
                    }
                    tbody {
                        for movement in page.movements {
                            tr {
                                td { "{movement.sequence}" }
                                td { "{movement.kind}" }
                                td { "{movement.lot_number.map(|value| value.to_string()).unwrap_or_else(|| \"—\".to_owned())}" }
                                td { "{movement.amount}" }
                                td { "{movement.balance_after}" }
                                td { "{movement.created_at}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
