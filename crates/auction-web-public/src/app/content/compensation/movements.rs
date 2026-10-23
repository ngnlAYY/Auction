//! Public 补偿流水详情及其金额/类型展示。

use super::*;
use auction_web_shared::{TruncatedText, format_held_duration};

#[component]
pub(super) fn CompensationMovementDetails(
    round: CompensationRound,
    movements: Option<Result<Option<CompensationMovementPage>, String>>,
    on_movement_page_change: EventHandler<i64>,
    on_movement_page_clamp: EventHandler<i64>,
    on_movement_page_size_change: EventHandler<i64>,
    on_retry_movements: EventHandler<()>,
) -> Element {
    let equation = t!(
        "compensation-equation",
        opening: format_amount(round.opening_balance),
        income: format_amount(round.tax_income),
        outflow: format_amount(round.compensation_out),
        closing: format_amount(round.closing_balance)
    );
    rsx! {
        section { class: "public-compensation-movements", aria_labelledby: "compensation-movements-title",
            h2 { id: "compensation-movements-title", class: "public-visually-hidden",
                {t!("compensation-movements")}
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
                    div { class: "public-compensation-movement-wrap ui-table--fit",
                        table { class: "public-compensation-movement-table",
                            auction_web_shared::TableColumns { layout: "public-compensation-movements", widths: vec![8, 28, 8, 10, 10, 12, 10, 14] }
                            thead {
                                tr {
                                    th { {t!("compensation-lot")} }
                                    th { {t!("compensation-equipment")} }
                                    th { {t!("compensation-world")} }
                                    th { class: "public-compensation-number", {t!("compensation-hold")} }
                                    th { class: "public-compensation-number", {t!("compensation-adjustment")} }
                                    th { class: "public-compensation-number", {t!("compensation-held-time")} }
                                    th { {t!("compensation-kind")} }
                                    th { class: "public-compensation-number", {t!("compensation-amount")} }
                                }
                            }
                            tbody {
                                // 期初、期末账本事实仍由 API 分页；页面只展示真实收支，避免重复占位行。
                                for movement in page.movements.into_iter().filter(|movement| !matches!(movement.kind.as_str(), "opening" | "closing")) {
                                    CompensationMovementRow { movement, world_id: round.world_id.clone() }
                                }
                            }
                            tfoot {
                                tr {
                                    td { colspan: 8, class: "public-compensation-equation",
                                        // 使用整期权威汇总，每一页都固定保留同一跨列末行。
                                        TruncatedText { value: equation, size: "wide", portal: true }
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
                        on_page_clamp: on_movement_page_clamp,
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
                        to: crate::app::surface::lot_detail_world_href(&lot_id, &world_id),
                        "{lot_code}"
                    }
                } else if let Some((lot_code, None)) = lot {
                    "{lot_code}"
                } else {
                    "—"
                }
            }
            td {
                if let Some(url) = movement.game_equipment_url.as_deref() {
                    a {
                        class: "public-compensation-equipment-link",
                        href: url,
                        target: "_blank",
                        rel: "noopener noreferrer",
                        TruncatedText { value: equipment, size: "wide", portal: true }
                    }
                } else {
                    TruncatedText { value: equipment, size: "wide", portal: true }
                }
            }
            td { "{world_label(&world_id)}" }
            td { class: "public-compensation-number",
                {movement.hold_ppm.map(|value| format_ppm(value, false)).unwrap_or_else(|| "—".to_owned())}
            }
            td { class: "public-compensation-number",
                {movement.adjustment_ppm.map(|value| format_ppm(value, true)).unwrap_or_else(|| "—".to_owned())}
            }
            td { class: "public-compensation-number",
                {movement
                    .leader_held_nanos
                    .map(format_held_duration)
                    .unwrap_or_else(|| "—".to_owned())}
            }
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

fn format_ppm(value: i32, show_positive_sign: bool) -> String {
    let absolute = i64::from(value).abs();
    let whole = absolute / 10_000;
    let fraction = absolute % 10_000;
    let number = if fraction == 0 {
        whole.to_string()
    } else {
        let fraction = format!("{fraction:04}").trim_end_matches('0').to_owned();
        format!("{whole}.{fraction}")
    };
    let sign = if value < 0 {
        "−"
    } else if value > 0 && show_positive_sign {
        "+"
    } else {
        ""
    };
    format!("{sign}{number}%")
}
