//! Public 补偿流水详情及其金额/类型展示。

use super::*;
use auction_web_shared::TruncatedText;

#[component]
pub(super) fn CompensationMovementDetails(
    round: CompensationRound,
    movements: Option<Result<Option<CompensationMovementPage>, String>>,
    on_movement_page_change: EventHandler<i64>,
    on_movement_page_size_change: EventHandler<i64>,
    on_retry_movements: EventHandler<()>,
) -> Element {
    rsx! {
        section { class: "public-compensation-movements", aria_labelledby: "compensation-movements-title",
            h2 { id: "compensation-movements-title", class: "public-visually-hidden",
                {t!("compensation-movements")}
            }
            p { class: "public-compensation-equation",
                {t!(
                    "compensation-equation",
                    opening: format_amount(round.opening_balance),
                    income: format_amount(round.tax_income),
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
                        div { class: "public-compensation-movement-wrap ui-table--fit",
                            table { class: "public-compensation-movement-table",
                                // 列序：拍品、装备、世界、类型、金额；按字段预期内容分配比例。
                                auction_web_shared::TableColumns { layout: "public-compensation-movements", widths: vec![8, 48, 10, 16, 18] }
                                thead {
                                    tr {
                                        th { {t!("compensation-lot")} }
                                        th { {t!("compensation-equipment")} }
                                        th { {t!("compensation-world")} }
                                        th { {t!("compensation-kind")} }
                                        th { class: "public-compensation-number", {t!("compensation-amount")} }
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
                        movement: true,
                        on_page_change: on_movement_page_change,
                        on_page_size_change: on_movement_page_size_change,
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
    let kind_class = if movement.amount > 0 {
        "public-compensation-income"
    } else if movement.amount < 0 {
        "public-compensation-outflow"
    } else {
        ""
    };
    let equipment =
        match (movement.equipment_name.as_deref(), movement.equipment_category.as_deref()) {
            (Some(name), Some(category)) => format!("{name}({category})"),
            (Some(name), None) => name.to_owned(),
            (None, Some(category)) => category.to_owned(),
            (None, None) => "—".to_owned(),
        };
    rsx! {
        tr {
            td {
                if let Some((lot_code, Some(lot_id))) = lot {
                    Link {
                        class: "public-compensation-lot-link",
                        to: Route::LotDetail { lot_code: lot_id },
                        "{lot_code}"
                    }
                } else if let Some((lot_code, None)) = lot {
                    "{lot_code}"
                } else {
                    "—"
                }
            }
            td {
                TruncatedText { value: equipment, size: "wide", portal: true }
            }
            td { "{world_label(&world_id)}" }
            td { span { class: kind_class, "{compensation_kind_label(&movement.kind)}" } }
            td { class: "public-compensation-number", "{amount}" }
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
