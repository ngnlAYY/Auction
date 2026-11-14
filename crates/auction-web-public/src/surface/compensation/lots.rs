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
    use_content_loading(lots.is_none());
    rsx! {
        section { class: "public-compensation-movements", aria_label: t!("compensation-lots"),
            match lots {
                None => rsx! {},
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
                Some(Ok(Some(page))) => {
                    // 冲正列默认隐藏，只有本页存在已撤销成交（含未冲正）时才显示。
                    let show_reversal =
                        page.lots.iter().any(|lot| lot.voided || lot.reversal.is_some());
                    rsx! {
                        if page.lots.is_empty() {
                            p { class: "public-auction-page__status", {t!("data-empty")} }
                        } else {
                            div { class: "public-compensation-movement-wrap ui-table--fit",
                                table { class: "public-compensation-movement-table",
                                    auction_web_shared::TableColumns { layout: "public-compensation-lots", widths: lot_column_widths(show_reversal) }
                                    thead {
                                        tr {
                                            th { {t!("compensation-lot")} }
                                            th { {t!("compensation-equipment")} }
                                            th { {t!("compensation-world")} }
                                            th { class: "public-compensation-number", {t!("compensation-hold")} }
                                            th { class: "public-compensation-number", {t!("compensation-adjustment")} }
                                            th { class: "public-compensation-number", {t!("compensation-held-time")} }
                                            th { class: "public-compensation-kind-cell", {t!("compensation-kind")} }
                                            th { class: "public-compensation-number", {t!("compensation-amount")} }
                                            th { class: "public-compensation-number", {t!("compensation-end-price")} }
                                            th { class: "public-compensation-number", {t!("compensation-buyer-amount")} }
                                            th { class: "public-compensation-number", {t!("compensation-seller-amount")} }
                                            if show_reversal {
                                                th { class: "public-compensation-number", {t!("compensation-reversal")} }
                                            }
                                            th { class: "public-lot-view-cell", {t!("catalog-actions")} }
                                        }
                                    }
                                    tbody {
                                        for lot in page.lots {
                                            CompensationLotRow {
                                                lot,
                                                auction_id: round.auction_id.clone(),
                                                world_id: round.world_id.clone(),
                                                show_reversal,
                                            }
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
                    }
                }
            }
        }
    }
}

#[component]
fn CompensationLotRow(
    lot: crate::api::CompensationLot,
    auction_id: String,
    world_id: String,
    show_reversal: bool,
) -> Element {
    let lot_code = lot_code_for(&lot.equipment_category, i64::from(lot.lot_number));
    let equipment = format!("{}({})", lot.equipment_name, lot.equipment_category);
    // 一件成交要么缴纳税收入池，要么从池中获得补偿，用“类型”一列区分；金额按池的视角带符号，
    // 税收流入为 `+`、补偿流出为 `−`。有折扣需求时即使补偿为零也显示，让池余额不足可见，
    // 名义折扣需求放进提示供复算。
    let (kind, kind_class, amount, amount_class, amount_title) = if lot.tax_amount > 0 {
        (
            Some(t!("compensation-kind-tax")),
            "public-compensation-kind public-compensation-kind--tax",
            signed_amount(lot.tax_amount),
            "public-compensation-income",
            None,
        )
    } else if lot.compensation_amount > 0 || lot.discount_amount > 0 {
        (
            Some(t!("compensation-kind-compensation")),
            "public-compensation-kind public-compensation-kind--compensation",
            signed_amount(-lot.compensation_amount),
            "public-compensation-outflow",
            Some(t!("compensation-discount-title", amount: credits(lot.discount_amount))),
        )
    } else {
        (None, "", "—".to_owned(), "", None)
    };
    let reversal = match lot.reversal.as_ref() {
        Some(reversal) if reversal.tax_shortfall > 0 => t!(
            "compensation-reversal-shortfall",
            amount: signed_amount(reversal.amount),
            shortfall: credits(reversal.tax_shortfall)
        ),
        Some(reversal) => signed_amount(reversal.amount),
        None if lot.voided => t!("compensation-voided-unreversed"),
        None => "—".to_owned(),
    };
    let reversal_title = lot.reversal.as_ref().map(|reversal| {
        t!(
            "compensation-reversal-detail",
            date: format_auction_time(&reversal.reversed_at),
            balance: credits(reversal.balance_after)
        )
    });
    rsx! {
        tr { class: if lot.voided { "public-compensation-voided" } else { "" },
            // 编号只作标识；拍品详情统一经操作栏的查看按钮在页内抽屉打开，不再跳转。
            td { "data-label": t!("compensation-lot"), "{lot_code}" }
            td { "data-label": t!("compensation-equipment"),
                a {
                    class: "public-compensation-equipment-link",
                    href: lot.game_equipment_url,
                    target: "_blank",
                    rel: "noopener noreferrer",
                    aria_label: equipment.clone(),
                    title: equipment,
                    TruncatedText { value: equipment.clone(), size: "wide", focusable: false, portal: true }
                }
            }
            td { "data-label": t!("compensation-world"), "{world_label(&world_id)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-hold"),
                {format_ppm(lot.hold_ppm, false)}
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-adjustment"),
                {format_ppm(lot.adjustment_ppm, true)}
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-held-time"),
                {format_held_duration(lot.leader_held_nanos)}
            }
            td { class: "public-compensation-kind-cell", "data-label": t!("compensation-kind"),
                if let Some(kind) = kind {
                    span { class: kind_class, title: kind, "{kind}" }
                } else {
                    "—"
                }
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-amount"),
                span { class: amount_class, title: amount_title, "{amount}" }
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-end-price"),
                "{credits(lot.end_price)}"
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-buyer-amount"),
                CompensationParty { amount: lot.buyer_amount, name: lot.buyer_name.clone(), strong: true }
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-seller-amount"),
                CompensationParty { amount: lot.seller_amount, name: lot.seller_name.clone(), strong: false }
            }
            if show_reversal {
                td { class: "public-compensation-number", "data-label": t!("compensation-reversal"),
                    span { title: reversal_title, "{reversal}" }
                }
            }
            td { class: "public-lot-view-cell", "data-label": t!("catalog-actions"),
                LotDetailLookupButton {
                    auction_id,
                    lot_id: lot.lot_id.clone(),
                    lot_code,
                    world: Some(world_id),
                }
            }
        }
    }
}

/// 买家实付/卖家所得格：金额始终完整显示，后接括号内的玩家名；名字过长时截断并提供完整值提示。
/// 没有投稿记录的历史 lot 卖家为 `—`，此时不显示括号。
#[component]
fn CompensationParty(amount: i64, name: String, strong: bool) -> Element {
    rsx! {
        span { class: "public-compensation-party",
            if strong {
                strong { "{credits(amount)}" }
            } else {
                span { "{credits(amount)}" }
            }
            if name != "—" {
                span { class: "public-compensation-party__name",
                    TruncatedText { value: name, size: "compact", focusable: false, portal: true }
                }
            }
        }
    }
}

/// 逐件明细列宽（千分比）。数字列按窄桌面（表宽约 1000px、小号字）下最长完整值实测预留，
/// 类型列容纳英文 “Compensation” 标签；买家实付/卖家所得在金额之外留出玩家名空间。
/// 装备名与玩家名取剩余宽度，是仅有的允许截断的内容。
fn lot_column_widths(show_reversal: bool) -> Vec<u16> {
    // 末列是查看按钮；权重按总和归一化，其余列等比例让出宽度。
    if show_reversal {
        vec![51, 89, 50, 75, 77, 87, 109, 80, 90, 118, 110, 64, 40]
    } else {
        vec![51, 113, 50, 75, 77, 87, 109, 80, 90, 138, 130, 40]
    }
}
