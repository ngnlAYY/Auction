//! Admin 结算 lot 表格、资金池摘要和分页展示。

use auction_web_shared::t;
use auction_web_shared::{
    AsyncState, AsyncStateKind, Time, TruncatedText, format_credit_amount, format_held_duration,
    format_ppm,
};
use dioxus::prelude::*;

use super::RecoveryConfirmationDraft;
use super::helpers::{page_count, page_slice, price_adjustment_label};
use super::recovery::AdminRecoveryTable;
use crate::api::{
    AdminRecoveryReport, AdminSettlementLot, AdminSettlementReport, AdminSettlementSummary,
};
use crate::surface::AdminActionFeedback;
use crate::surface::pagination::{AdminPager as Pager, AdminPagination, use_admin_pagination};

#[component]
pub(super) fn AdminSettlementReportView(
    report: AdminSettlementReport,
    recovery: Option<AdminRecoveryReport>,
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
    action_busy: Signal<bool>,
    confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    reason: Signal<String>,
    action_feedback: Signal<Option<AdminActionFeedback>>,
) -> Element {
    let lot_pagination =
        use_admin_pagination("settlement-lots", &["lot_page_size"], None, i64::MAX);
    let recovery_pagination =
        use_admin_pagination("settlement-recovery", &["recovery_page_size"], None, i64::MAX);
    let lot_total_pages = page_count(report.lots.len(), lot_pagination.size());
    let current_lot_page = lot_page().clamp(1, lot_total_pages);
    let lot_rows = page_slice(&report.lots, current_lot_page, lot_pagination.size());
    let recovery_total_pages = recovery
        .as_ref()
        .map_or(1, |value| page_count(value.recoveries.len(), recovery_pagination.size()));
    let current_recovery_page = recovery_page().clamp(1, recovery_total_pages);

    rsx! {
        div { class: "admin-settlement__table-wrap",
            table { class: "admin-shell__table admin-auction-table",
                    aria_label: t!("page-settlement-title"),
                    auction_web_shared::TableColumns { layout: "admin-settlement", widths: vec![7, 20, 11, 9, 10, 10, 13, 9, 11] }
                    thead {
                        tr {
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-lot-id")} }
                            th { scope: "col", {t!("admin-settlement-equipment")} }
                            th { scope: "col",
                                {stacked_pair(
                                    rsx! { {t!("admin-settlement-seller")} },
                                    rsx! { {t!("admin-settlement-winner")} },
                                )}
                            }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-final-price")} }
                            th { class: "admin-auction-num", scope: "col",
                                {stacked_pair(
                                    rsx! { {t!("admin-settlement-hold")} },
                                    rsx! { {t!("admin-settlement-adjustment")} },
                                )}
                            }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-held-time")} }
                            th { class: "admin-auction-num", scope: "col",
                                {stacked_pair(
                                    rsx! { {t!("admin-settlement-discount-penalty")} },
                                    rsx! { {t!("admin-settlement-listed-price")} },
                                )}
                            }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-compensation")} }
                            th { class: "admin-auction-num", scope: "col",
                                {stacked_pair(
                                    rsx! { {t!("admin-settlement-buyer")} },
                                    rsx! { {t!("admin-settlement-seller-amount")} },
                                )}
                            }
                        }
                    }
                    tbody {
                        for lot in lot_rows {
                            AdminSettlementLotRow { lot }
                        }
                    }
                }
        }
        if report.lots.is_empty() {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-settlement-no-lots"),
                detail: "",
            }
        }
        AdminSettlementPager {
            current_page: lot_page(),
            total_rows: report.lots.len(),
            pagination: lot_pagination,
            page: lot_page,
            label: t!("admin-settlement-lots-pagination"),
        }
        AdminSettlementSummaryCard { summary: report.settlement }
        AdminRecoveryTable {
            report: recovery,
            current_page: current_recovery_page,
            pagination: recovery_pagination,
            page: recovery_page,
            action_busy,
            confirmation,
            reason,
            action_feedback,
        }
    }
}

#[component]
fn AdminSettlementLotRow(lot: AdminSettlementLot) -> Element {
    let hold_share = format_ppm(lot.hold_ppm, false);
    let adjustment_rate = format_ppm(lot.adjustment_ppm, true);
    let held_time = format_held_duration(lot.leader_held_nanos);
    let price_adjustment = price_adjustment_label(&lot);
    let equipment_name = lot.equipment_name.clone();
    let equipment_url = lot.game_equipment_url.clone();
    let lot_code = super::super::lots::admin_lot_code_for(&lot.equipment_category, lot.lot_number);
    let seller = lot.seller.unwrap_or_else(|| t!("value-empty"));
    let winner = lot.winner.unwrap_or_else(|| t!("value-empty"));
    rsx! {
        tr {
            td { class: "admin-auction-num", "{lot_code}" }
            td {
                // 装备名本身即游戏装备链接，最多两行、超出由 CSS 截断；DOM 与 title 保留全文，
                // 读屏名称以可见装备名为主体并说明跳转动作。API 层已拒绝无效装备 URL。
                a {
                    class: "admin-settlement-equipment-link",
                    href: equipment_url,
                    target: "_blank",
                    rel: "noopener noreferrer",
                    title: equipment_name.clone(),
                    aria_label: t!("admin-settlement-open-equipment-label", equipment: equipment_name),
                    "{equipment_name}"
                }
            }
            td {
                {stacked_pair(
                    rsx! { TruncatedText { value: seller } },
                    rsx! { TruncatedText { value: winner } },
                )}
            }
            td { class: "admin-auction-num", {format_credit_amount(lot.final_price)} }
            td { class: "admin-auction-num",
                {stacked_pair(rsx! { "{hold_share}" }, rsx! { "{adjustment_rate}" })}
            }
            td { class: "admin-auction-num", "{held_time}" }
            td { class: "admin-auction-num",
                {stacked_pair(
                    rsx! { "{price_adjustment}" },
                    rsx! { {format_credit_amount(lot.listed_price)} },
                )}
            }
            td { class: "admin-auction-num", {format_credit_amount(lot.compensation_amount)} }
            td { class: "admin-auction-num",
                {stacked_pair(
                    rsx! { {format_credit_amount(lot.buyer_amount)} },
                    rsx! { {format_credit_amount(lot.seller_amount)} },
                )}
            }
        }
    }
}

/// 相邻两个字段共用一列：上行是原顺序中的前一字段，下行是后一字段，表头与单元格同构。
/// 十三个字段单行至少需要约 1400px，合并成对字段后桌面报告卡内可完整显示金额与比例。
fn stacked_pair(primary: Element, secondary: Element) -> Element {
    rsx! {
        span { class: "admin-settlement__line", {primary} }
        span { class: "admin-settlement__line admin-settlement__line--secondary", {secondary} }
    }
}

#[component]
fn AdminSettlementSummaryCard(summary: Option<AdminSettlementSummary>) -> Element {
    rsx! {
        section { class: "admin-auction-report", aria_labelledby: "settlement-pool-title",
            h2 { id: "settlement-pool-title", {t!("admin-settlement-summary-title")} }
            if let Some(summary) = summary {
                div { class: "admin-auction-cards",
                    SettlementMetric { label: t!("admin-settlement-tax-income"), value: summary.tax_income }
                    SettlementMetric { label: t!("admin-settlement-discount"), value: summary.discount_demand }
                    SettlementMetric { label: t!("admin-settlement-compensation-out"), value: summary.compensation_out }
                    SettlementMetric { label: t!("admin-settlement-closing"), value: summary.closing_balance }
                    SettlementMetric { label: t!("admin-settlement-opening"), value: summary.opening_balance }
                }
                p { class: "admin-auction-status",
                    {t!("admin-settlement-summary-meta")}
                    " "
                    Time { value: summary.settled_at }
                }
            } else {
                p { class: "admin-auction-status", {t!("admin-settlement-not-settled")} }
            }
        }
    }
}

#[component]
fn SettlementMetric(label: String, value: i64) -> Element {
    rsx! {
        div { class: "admin-auction-card",
            div { class: "admin-auction-card-value", {format_credit_amount(value)} }
            div { class: "admin-auction-card-label", {label} }
        }
    }
}

#[component]
pub(super) fn AdminSettlementPager(
    current_page: i64,
    total_rows: usize,
    pagination: AdminPagination,
    mut page: Signal<i64>,
    label: String,
) -> Element {
    rsx! {
        Pager {
            page: current_page,
            page_size: pagination.size(),
            pagination,
            local: true,
            total: Some(i64::try_from(total_rows).unwrap_or(i64::MAX)),
            label,
            on_page_change: move |next_page| page.set(next_page),
        }
    }
}
