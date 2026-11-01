//! Public 单场逐件结算明细：每件的计算依据、金额与成交撤销冲正。

use super::*;
use auction_web_shared::{TruncatedText, format_held_duration, format_ppm};

#[component]
pub(super) fn CompensationLotDetails(
    round: CompensationRound,
    lots: Option<Result<Option<CompensationLotPage>, String>>,
    on_lot_page_change: EventHandler<i64>,
    on_lot_page_clamp: EventHandler<i64>,
    on_lot_page_size_change: EventHandler<i64>,
    on_retry_lots: EventHandler<()>,
) -> Element {
    // 可用额 = 期初余额 + 本场税收；与兑付率一起给出逐件补偿的复算输入。
    let available = round.opening_balance.saturating_add(round.tax_income);
    rsx! {
        section { class: "public-compensation-movements", aria_labelledby: "compensation-lots-title",
            h2 { id: "compensation-lots-title", class: "public-visually-hidden",
                {t!("compensation-lots")}
            }
            p { class: "public-compensation-summary",
                {t!(
                    "compensation-round-summary",
                    policy: policy_label(&round.policy_mode),
                    available: format_amount(available),
                    coverage: coverage_label(round.compensation_out, round.discount_demand)
                )}
            }
            match lots {
                None => rsx! { p { class: "public-auction-page__status", role: "status", {t!("data-loading")} } },
                Some(Err(_)) => rsx! {
                    div { role: "alert",
                        p { class: "public-auction-page__status", {t!("data-error")} }
                        button {
                            r#type: "button",
                            onclick: move |_| on_retry_lots.call(()),
                            {t!("data-retry")}
                        }
                    }
                },
                Some(Ok(None)) => rsx! { p { class: "public-auction-page__status", {t!("data-empty")} } },
                Some(Ok(Some(page))) => rsx! {
                    if page.lots.is_empty() {
                        p { class: "public-auction-page__status", {t!("data-empty")} }
                    } else {
                        div { class: "public-compensation-movement-wrap ui-table--fit",
                            table { class: "public-compensation-movement-table",
                                auction_web_shared::TableColumns { layout: "public-compensation-lots", widths: vec![6, 20, 6, 6, 8, 8, 7, 7, 7, 8, 8, 9] }
                                thead {
                                    tr {
                                        th { {t!("compensation-lot")} }
                                        th { {t!("compensation-equipment")} }
                                        th { class: "public-compensation-number", {t!("compensation-hold")} }
                                        th { class: "public-compensation-number", {t!("compensation-adjustment")} }
                                        th { class: "public-compensation-number", {t!("compensation-held-time")} }
                                        th { class: "public-compensation-number", {t!("compensation-end-price")} }
                                        th { class: "public-compensation-number", {t!("compensation-kind-tax")} }
                                        th { class: "public-compensation-number", {t!("compensation-discount")} }
                                        th { class: "public-compensation-number", {t!("compensation-kind-compensation")} }
                                        th { class: "public-compensation-number", {t!("compensation-buyer-amount")} }
                                        th { class: "public-compensation-number", {t!("compensation-seller-amount")} }
                                        th { class: "public-compensation-number", {t!("compensation-reversal")} }
                                    }
                                }
                                tbody {
                                    for lot in page.lots {
                                        CompensationLotRow { lot, world_id: round.world_id.clone() }
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
                        label: t!("compensation-lots"),
                        movement: true,
                        on_page_change: on_lot_page_change,
                        on_page_clamp: on_lot_page_clamp,
                        on_page_size_change: on_lot_page_size_change,
                    }
                },
            }
        }
    }
}

#[component]
fn CompensationLotRow(lot: crate::api::CompensationLot, world_id: String) -> Element {
    let lot_code = lot_code_for(&lot.equipment_category, i64::from(lot.lot_number));
    let equipment = format!("{}({})", lot.equipment_name, lot.equipment_category);
    let tax = (lot.tax_amount > 0).then(|| format_amount(lot.tax_amount));
    let discount = (lot.discount_amount > 0).then(|| format_amount(lot.discount_amount));
    // 有优惠需求时即使补偿为零也显示 0，让池余额不足的情况可见。
    let compensation = (lot.compensation_amount > 0 || lot.discount_amount > 0)
        .then(|| format_amount(lot.compensation_amount));
    let reversal = match lot.reversal.as_ref() {
        Some(reversal) if reversal.tax_shortfall > 0 => t!(
            "compensation-reversal-shortfall",
            amount: signed_amount(reversal.amount),
            shortfall: format_amount(reversal.tax_shortfall)
        ),
        Some(reversal) => signed_amount(reversal.amount),
        None if lot.voided => t!("compensation-voided-unreversed"),
        None => "—".to_owned(),
    };
    let reversal_title = lot.reversal.as_ref().map(|reversal| {
        t!(
            "compensation-reversal-detail",
            date: format_auction_time(&reversal.reversed_at),
            balance: format_amount(reversal.balance_after)
        )
    });
    rsx! {
        tr { class: if lot.voided { "public-compensation-voided" } else { "" },
            td { "data-label": t!("compensation-lot"),
                Link {
                    class: "public-compensation-lot-link",
                    to: crate::app::surface::lot_detail_world_href(&lot.lot_id, &world_id),
                    "{lot_code}"
                }
            }
            td { "data-label": t!("compensation-equipment"),
                a {
                    class: "public-compensation-equipment-link",
                    href: lot.game_equipment_url.clone(),
                    target: "_blank",
                    rel: "noopener noreferrer",
                    aria_label: equipment.clone(),
                    title: equipment.clone(),
                    TruncatedText { value: equipment.clone(), size: "wide", focusable: false, portal: true }
                }
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-hold"),
                {format_ppm(lot.hold_ppm, false)}
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-adjustment"),
                {format_ppm(lot.adjustment_ppm, true)}
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-held-time"),
                {format_held_duration(lot.leader_held_nanos)}
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-end-price"),
                "{format_amount(lot.end_price)}"
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-kind-tax"),
                if let Some(tax) = tax {
                    span { class: "public-compensation-income", "{tax}" }
                } else {
                    "—"
                }
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-discount"),
                {discount.unwrap_or_else(|| "—".to_owned())}
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-kind-compensation"),
                if let Some(compensation) = compensation {
                    span { class: "public-compensation-outflow", "{compensation}" }
                } else {
                    "—"
                }
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-buyer-amount"),
                "{format_amount(lot.buyer_amount)}"
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-seller-amount"),
                "{format_amount(lot.seller_amount)}"
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-reversal"),
                span { title: reversal_title, "{reversal}" }
            }
        }
    }
}
