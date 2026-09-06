//! Admin 结算 lot 表格、资金池摘要和分页展示。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{
    AsyncState, AsyncStateKind, Pager, Time, TruncatedText, format_credit_amount as format_amount,
};

use crate::api::{
    AdminRecoveryReport, AdminSettlementLot, AdminSettlementReport, AdminSettlementSummary,
};
use crate::app::surface::AdminActionFeedback;

use super::RecoveryConfirmationDraft;
use super::helpers::{format_ppm, page_count, page_slice, price_adjustment_label};
use super::recovery::AdminRecoveryTable;

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
    let lot_total_pages = page_count(report.lots.len());
    let current_lot_page = lot_page().clamp(1, lot_total_pages as i64);
    let lot_rows = page_slice(&report.lots, current_lot_page);
    let recovery_total_pages =
        recovery.as_ref().map_or(1, |value| page_count(value.recoveries.len()));
    let current_recovery_page = recovery_page().clamp(1, recovery_total_pages as i64);

    rsx! {
        div { class: "admin-settlement__table-wrap",
            table { class: "admin-shell__table admin-auction-table",
                    caption { class: "admin-visually-hidden", {t!("page-settlement-title")} }
                    // 列序：编号、装备、卖家、得主、成交价、持有比例、调整比例、折扣/惩罚、挂牌价、补偿、买家应付、卖家应收；按字段预期内容分配比例。
                    auction_web_shared::TableColumns { layout: "admin-settlement", widths: vec![6, 18, 7, 5, 9, 8, 7, 9, 9, 5, 10, 7] }
                    thead {
                        tr {
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-lot-id")} }
                            th { scope: "col", {t!("admin-settlement-equipment")} }
                            th { scope: "col", {t!("admin-settlement-seller")} }
                            th { scope: "col", {t!("admin-settlement-winner")} }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-final-price")} }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-hold")} }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-adjustment")} }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-discount-penalty")} }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-listed-price")} }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-compensation")} }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-buyer")} }
                            th { class: "admin-auction-num", scope: "col", {t!("admin-settlement-seller-amount")} }
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
            current_page: current_lot_page,
            total_pages: lot_total_pages,
            page: lot_page,
            label: t!("admin-settlement-lots-pagination"),
        }
        AdminSettlementSummaryCard { summary: report.settlement.clone() }
        AdminRecoveryTable {
            report: recovery,
            current_page: current_recovery_page,
            total_pages: recovery_total_pages,
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
    let price_adjustment = price_adjustment_label(&lot);
    let equipment_name = lot.equipment_name.clone();
    let lot_code = super::super::lots::admin_lot_code_for(&lot.equipment_category, lot.lot_number);
    let seller = lot.seller.unwrap_or_else(|| t!("value-empty"));
    let winner = lot.winner.unwrap_or_else(|| t!("value-empty"));
    rsx! {
        tr {
            td { class: "admin-auction-num", "{lot_code}" }
            td { TruncatedText { value: equipment_name } }
            td { TruncatedText { value: seller } }
            td { TruncatedText { value: winner } }
            td { class: "admin-auction-num", {format_amount(lot.final_price)} }
            td { class: "admin-auction-num", "{hold_share}" }
            td { class: "admin-auction-num", "{adjustment_rate}" }
            td { class: "admin-auction-num", "{price_adjustment}" }
            td { class: "admin-auction-num", {format_amount(lot.listed_price)} }
            td { class: "admin-auction-num", {format_amount(lot.compensation_amount)} }
            td { class: "admin-auction-num", {format_amount(lot.buyer_amount)} }
            td { class: "admin-auction-num", {format_amount(lot.seller_amount)} }
        }
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
                    Time { value: summary.settled_at.clone() }
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
            div { class: "admin-auction-card-value", {format_amount(value)} }
            div { class: "admin-auction-card-label", {label} }
        }
    }
}

#[component]
pub(super) fn AdminSettlementPager(
    current_page: i64,
    total_pages: usize,
    mut page: Signal<i64>,
    label: String,
) -> Element {
    rsx! {
        Pager {
            page: current_page,
            page_size: 1,
            total: Some(i64::try_from(total_pages).unwrap_or(i64::MAX)),
            label,
            previous_label: t!("admin-pagination-previous"),
            status: move |(page, pages): (i64, i64)| {
                t!("admin-pagination-status", page: page, pages: pages)
            },
            next_label: t!("admin-pagination-next"),
            jump_label: t!("admin-pagination-input"),
            jump_button_label: t!("admin-pagination-go"),
            on_page_change: move |next_page| page.set(next_page),
        }
    }
}
