//! Public 补偿池与补偿账本页面。
//!
//! 页面保留旧版的批次分页、展开流水和分页读取行为。所有金额与时间都通过公开端已有
//! 的格式化边界处理，浏览器不推导补偿池状态，也不把分页总数当作实时余额。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    CompensationMovementPage, CompensationPoolPage, CompensationRound,
    fetch_compensation_movements, fetch_compensation_pool, lot_code_for,
};

use super::pagination::PublicPager;
use super::{
    Placeholder, RetryPlaceholder, Route, format_amount, format_auction_time, read_public_query,
    update_public_query, world_label,
};

/// 读取公开补偿池批次，并按旧版公开页面展示流水详情。
pub(super) fn pool_page() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let mut page_number = use_signal(|| read_public_query().page.max(1));
    let mut selected_auction_id = use_signal(String::new);
    let mut movement_page = use_signal(|| 1_i64);
    let pool = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            fetch_compensation_pool(page_number())
        }
    });
    let movements = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let auction_id = selected_auction_id();
            let page = movement_page();
            async move {
                if auction_id.is_empty() {
                    Ok(None)
                } else {
                    fetch_compensation_movements(auction_id, page).await.map(Some)
                }
            }
        }
    });

    match &*pool.read_unchecked() {
        None => rsx! {
            Placeholder {
                title: t!("page-compensation-pool-title"),
                detail: t!("data-loading"),
            }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-compensation-pool-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(page)) => {
            let movement_state = movements.read_unchecked().clone();
            rsx! {
                CompensationPoolTable {
                    page: page.clone(),
                    selected_auction_id: selected_auction_id(),
                    movements: movement_state,
                    on_page_change: move |next_page: i64| {
                        let next_page = next_page.max(1);
                        page_number.set(next_page);
                        selected_auction_id.set(String::new());
                        movement_page.set(1);
                        update_public_query(|query| query.page = next_page, false);
                    },
                    on_select_round: move |auction_id: String| {
                        if selected_auction_id() == auction_id {
                            selected_auction_id.set(String::new());
                        } else {
                            selected_auction_id.set(auction_id);
                        }
                        movement_page.set(1);
                    },
                    on_movement_page_change: move |next_page: i64| {
                        movement_page.set(next_page.max(1));
                    },
                    on_retry_movements: move |_| {
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    },
                }
            }
        }
    }
}

#[component]
fn CompensationPoolTable(
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

#[component]
fn CompensationMovementDetails(
    round: CompensationRound,
    movements: Option<Result<Option<CompensationMovementPage>, String>>,
    on_movement_page_change: EventHandler<i64>,
    on_retry_movements: EventHandler<()>,
) -> Element {
    rsx! {
        section { class: "public-compensation-movements", aria_labelledby: "compensation-movements-title",
            h2 { id: "compensation-movements-title", class: "public-visually-hidden",
                {t!("compensation-movements")}
            }
            p { class: "public-auction-page__status",
                {t!(
                    "compensation-equation",
                    opening: format_amount(round.opening_balance),
                    income: format_amount(round.tax_income),
                    discount: format_amount(round.discount_demand),
                    outflow: format_amount(round.compensation_out),
                    closing: format_amount(round.closing_balance)
                )}
            }
            match movements {
                None => rsx! { p { class: "public-auction-page__status", role: "status", {t!("data-loading")} } },
                Some(Err(_)) => rsx! {
                    div { role: "alert",
                        p { class: "public-auction-page__status", {t!("data-error")} }
                        button {
                            r#type: "button",
                            onclick: move |_| on_retry_movements.call(()),
                            {t!("data-retry")}
                        }
                    }
                },
                Some(Ok(None)) => rsx! { p { class: "public-auction-page__status", {t!("data-empty")} } },
                Some(Ok(Some(page))) => rsx! {
                    if page.movements.is_empty() {
                        p { class: "public-auction-page__status", {t!("data-empty")} }
                    } else {
                        div { class: "public-auction-catalog",
                            table {
                                thead {
                                    tr {
                                        th { {t!("compensation-lot")} }
                                        th { {t!("compensation-equipment")} }
                                        th { {t!("compensation-world")} }
                                        th { {t!("compensation-kind")} }
                                        th { {t!("compensation-amount")} }
                                    }
                                }
                                tbody {
                                    for movement in page.movements {
                                        CompensationMovementRow { movement, world_id: round.world_id.clone() }
                                    }
                                }
                            }
                        }
                    }
                    PublicPager {
                        page: page.page,
                        page_size: page.page_size,
                        total: Some(page.total),
                        has_more: false,
                        label: t!("compensation-movements"),
                        on_page_change: on_movement_page_change,
                    }
                },
            }
        }
    }
}

#[component]
fn CompensationMovementRow(
    movement: crate::api::CompensationMovement,
    world_id: String,
) -> Element {
    let lot =
        movement.lot_number.zip(movement.equipment_category.clone()).map(|(number, category)| {
            (lot_code_for(&category, i64::from(number)), movement.lot_id.clone())
        });
    let amount = if movement.amount > 0 {
        format!("+{}", format_amount(movement.amount))
    } else if movement.amount < 0 {
        format!("−{}", format_amount(movement.amount.saturating_abs()))
    } else {
        format_amount(0)
    };
    rsx! {
        tr {
            td {
                if let Some((lot_code, Some(_lot_id))) = lot {
                    Link { to: Route::LotDetail { lot_code: lot_code.clone() }, "{lot_code}" }
                } else if let Some((lot_code, None)) = lot {
                    "{lot_code}"
                } else {
                    "—"
                }
            }
            td {
                if let Some(name) = movement.equipment_name {
                    "{name}"
                } else {
                    "—"
                }
                if let Some(category) = movement.equipment_category {
                    small { "{category}" }
                }
            }
            td { "{world_label(&world_id)}" }
            td { "{compensation_kind_label(&movement.kind)}" }
            td { class: "public-auction-catalog__amount", "{amount}" }
        }
    }
}

fn compensation_kind_label(kind: &str) -> String {
    match kind {
        "opening" => t!("compensation-kind-opening"),
        "tax" => t!("compensation-kind-tax"),
        "compensation" => t!("compensation-kind-compensation"),
        "closing" => t!("compensation-kind-closing"),
        _ => kind.to_owned(),
    }
}
