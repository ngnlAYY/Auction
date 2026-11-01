//! Public 补偿池余额、批次表格和展开行。

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
    // 冲正列默认隐藏，只有本页存在冲正时才显示；否则整列都是“—”。
    let show_reversal = page.rounds.iter().any(|round| round.reversal_count > 0);
    rsx! {
        section { class: "public-compensation-page", aria_labelledby: "compensation-pool-title",
            header { class: "public-compensation-heading",
                h1 { id: "compensation-pool-title", {t!("page-compensation-pool-title")} }
                p { class: "public-compensation-intro", {t!("page-compensation-pool-note")} }
            }
            // 余额在前、计算方式在后：手机上先看到结果，桌面并排展示。
            div { class: "public-compensation-overview",
                div { class: "public-compensation-balance", aria_label: t!("compensation-balance"),
                    span { class: "public-compensation-balance__label", {t!("compensation-balance")} }
                    strong { class: "public-compensation-balance__amount", "{credits(page.balance)}" }
                    small {
                        time {
                            datetime: page.updated_at.clone(),
                            {t!("compensation-updated", date: format_auction_time(&page.updated_at))}
                        }
                    }
                }
                section { class: "public-compensation-rules", aria_labelledby: "compensation-rules-title",
                    h2 { id: "compensation-rules-title", {t!("compensation-rules-title")} }
                    ol {
                        li { {t!("compensation-rule-available")} }
                        li { {t!("compensation-rule-share")} }
                        li { {t!("compensation-rule-reversal")} }
                    }
                }
            }
            if page.rounds.is_empty() {
                p { class: "public-compensation-empty", {t!("compensation-empty")} }
            } else {
                div { class: "public-compensation-table-wrap ui-table--fit",
                    table { class: "public-compensation-table",
                        auction_web_shared::TableColumns { layout: "public-compensation", widths: round_column_widths(show_reversal) }
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
                                if show_reversal {
                                    th { scope: "col", class: "public-compensation-number", {t!("compensation-reversal")} }
                                }
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
                                            show_reversal,
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
    show_reversal: bool,
    lots: Option<Result<Option<CompensationLotPage>, String>>,
    on_select_round: EventHandler<String>,
    on_lot_page_change: EventHandler<i64>,
    on_lot_page_clamp: EventHandler<i64>,
    on_lot_page_size_change: EventHandler<i64>,
    on_retry_lots: EventHandler<()>,
) -> Element {
    let details_id = format!("compensation-details-{}", round.auction_id);
    let auction_id = round.auction_id.clone();
    // 单元格只放净额，笔数放进提示，避免窄列截断金额。
    let (reversal, reversal_title) = if round.reversal_count > 0 {
        let amount = signed_amount(round.reversal_amount);
        let title = t!(
            "compensation-reversal-summary",
            amount: amount.clone(),
            count: round.reversal_count
        );
        (amount, Some(title))
    } else {
        ("—".to_owned(), None)
    };
    rsx! {
        tr { class: if open { "public-compensation-round public-compensation-round--open" } else { "public-compensation-round" },
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
                    span { class: "public-chevron", aria_hidden: "true" }
                }
            }
            td { "data-label": t!("compensation-round-label"), "#{round.auction_sequence}" }
            td { "data-label": t!("compensation-world"), "{world_label(&round.world_id)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-opening"), "{credits(round.opening_balance)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-tax"),
                span { class: "public-compensation-income", "{signed_amount(round.tax_income)}" }
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-discount"), "{credits(round.discount_demand)}" }
            td { class: "public-compensation-number", "data-label": t!("compensation-out"),
                span { class: "public-compensation-outflow", "{signed_amount(-round.compensation_out)}" }
            }
            td { class: "public-compensation-number", "data-label": t!("compensation-coverage"),
                "{coverage_label(round.compensation_out, round.discount_demand)}"
            }
            td { class: "public-compensation-number public-compensation-closing", "data-label": t!("compensation-closing"),
                strong { "{credits(round.closing_balance)}" }
            }
            if show_reversal {
                td { class: "public-compensation-number", "data-label": t!("compensation-reversal"),
                    span { title: reversal_title, "{reversal}" }
                }
            }
            td { "data-label": t!("compensation-settled"),
                Time { value: round.settled_at.clone() }
            }
        }
        if open {
            tr { id: details_id.clone(), class: "public-compensation-details-row",
                td { colspan: if show_reversal { 11 } else { 10 },
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

/// 批次表列宽；隐藏冲正列时其余列按原权重归一化。
fn round_column_widths(show_reversal: bool) -> Vec<u16> {
    if show_reversal {
        vec![3, 6, 7, 10, 10, 9, 10, 7, 10, 10, 18]
    } else {
        vec![3, 6, 8, 10, 10, 9, 10, 7, 10, 19]
    }
}
