//! Admin 结算 lot 表格、资金池摘要和分页展示。

use dioxus::prelude::*;
use dioxus_i18n::t;

use auction_web_shared::{TruncatedText, format_credit_amount as format_amount};

use crate::api::{
    AdminRecoveryReport, AdminSettlementLot, AdminSettlementReport, AdminSettlementSummary,
};

use super::super::{format_admin_time, pagination::AdminPager};
use super::RecoveryConfirmationDraft;
use super::helpers::{auction_world_label, hold_share, page_count, page_slice};
use super::recovery::AdminRecoveryTable;

#[component]
pub(super) fn AdminSettlementReportView(
    report: AdminSettlementReport,
    recovery: Option<Result<Option<AdminRecoveryReport>, String>>,
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
    confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    reason: Signal<String>,
    action_feedback: Signal<Option<bool>>,
) -> Element {
    let lot_total_pages = page_count(report.lots.len());
    let current_lot_page = lot_page().clamp(1, lot_total_pages as i64);
    let lot_rows = page_slice(&report.lots, current_lot_page);
    let recovery_report = recovery.and_then(|value| value.ok()).flatten();
    let recovery_total_pages =
        recovery_report.as_ref().map_or(1, |value| page_count(value.recoveries.len()));
    let current_recovery_page = recovery_page().clamp(1, recovery_total_pages as i64);

    rsx! {
        section { class: "admin-settlement__report", aria_labelledby: "admin-settlement-report-title",
            h2 { id: "admin-settlement-report-title",
                {t!("admin-settlement-report-title", world: auction_world_label(&report.auction.world_id), sequence: report.auction.sequence)}
            }
            div { class: "admin-settlement__table-wrap",
                table {
                    caption { class: "admin-visually-hidden", {t!("admin-settlement-lots-title")} }
                    thead {
                        tr {
                            th { {t!("admin-settlement-lot")} }
                            th { {t!("admin-settlement-equipment")} }
                            th { {t!("admin-settlement-seller")} }
                            th { {t!("admin-settlement-winner")} }
                            th { {t!("admin-settlement-final-price")} }
                            th { {t!("admin-settlement-tax")} }
                            th { {t!("admin-settlement-hold")} }
                            th { {t!("admin-settlement-adjustment")} }
                            th { {t!("admin-settlement-listed-price")} }
                            th { {t!("admin-settlement-compensation")} }
                            th { {t!("admin-settlement-buyer")} }
                            th { {t!("admin-settlement-seller-amount")} }
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
                p { class: "admin-panel__note", {t!("admin-settlement-no-lots")} }
            }
            AdminSettlementPager {
                current_page: current_lot_page,
                total_pages: lot_total_pages,
                page: lot_page,
                label: t!("admin-settlement-lots-pagination"),
            }
            AdminSettlementSummaryCard { summary: report.settlement.clone() }
            if let Some(recovery_report) = recovery_report {
                AdminRecoveryTable {
                    report: recovery_report,
                    current_page: current_recovery_page,
                    total_pages: recovery_total_pages,
                    page: recovery_page,
                    confirmation,
                    reason,
                    action_feedback,
                }
            }
        }
    }
}

#[component]
fn AdminSettlementLotRow(lot: AdminSettlementLot) -> Element {
    let equipment_name = lot.equipment_name.clone();
    let seller = lot.seller.unwrap_or_else(|| t!("value-empty"));
    let winner = lot.winner.unwrap_or_else(|| t!("value-empty"));
    let adjustment = lot.adjustment.policy.unwrap_or_else(|| t!("value-empty"));
    let hold_share = hold_share(lot.tax_amount, lot.final_price);
    rsx! {
        tr { class: "admin-settlement__row",
            td { "data-label": t!("admin-settlement-lot"), "{lot.lot_number}" }
            td { "data-label": t!("admin-settlement-equipment"), TruncatedText { value: equipment_name } }
            td { "data-label": t!("admin-settlement-seller"), TruncatedText { value: seller } }
            td { "data-label": t!("admin-settlement-winner"), TruncatedText { value: winner } }
            td { "data-label": t!("admin-settlement-final-price"), class: "admin-settlement__amount", {format_amount(lot.final_price)} }
            td { "data-label": t!("admin-settlement-tax"), class: "admin-settlement__amount", {format_amount(lot.tax_amount)} }
            td { "data-label": t!("admin-settlement-hold"), class: "admin-settlement__amount", "{hold_share}" }
            td { "data-label": t!("admin-settlement-adjustment"), TruncatedText { value: adjustment } }
            td { "data-label": t!("admin-settlement-listed-price"), class: "admin-settlement__amount", {t!("value-empty")} }
            td { "data-label": t!("admin-settlement-compensation"), class: "admin-settlement__amount", {format_amount(lot.compensation_amount)} }
            td { "data-label": t!("admin-settlement-buyer"), class: "admin-settlement__amount", {format_amount(lot.buyer_amount)} }
            td { "data-label": t!("admin-settlement-seller-amount"), class: "admin-settlement__amount", {format_amount(lot.seller_amount)} }
        }
    }
}

#[component]
fn AdminSettlementSummaryCard(summary: Option<AdminSettlementSummary>) -> Element {
    rsx! {
        section { class: "admin-settlement__summary", aria_labelledby: "admin-settlement-summary-title",
            h2 { id: "admin-settlement-summary-title", {t!("admin-settlement-summary-title")} }
            if let Some(summary) = summary {
                div { class: "admin-settlement__cards",
                    SettlementMetric { label: t!("admin-settlement-opening"), value: summary.opening_balance }
                    SettlementMetric { label: t!("admin-settlement-tax-income"), value: summary.tax_income }
                    SettlementMetric { label: t!("admin-settlement-discount"), value: summary.discount_demand }
                    SettlementMetric { label: t!("admin-settlement-compensation-out"), value: summary.compensation_out }
                    SettlementMetric { label: t!("admin-settlement-closing"), value: summary.closing_balance }
                }
                p { class: "admin-panel__note",
                    {t!(
                        "admin-settlement-summary-meta",
                        movements: summary.movement_count,
                        at: format_admin_time(&summary.settled_at)
                    )}
                }
            } else {
                p { class: "admin-panel__note", {t!("admin-settlement-not-settled")} }
            }
        }
    }
}

#[component]
fn SettlementMetric(label: String, value: i64) -> Element {
    rsx! {
        div { class: "admin-settlement__metric",
            strong { {format_amount(value)} }
            span { {label} }
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
        AdminPager {
            page: current_page,
            page_size: 1,
            total: i64::try_from(total_pages).unwrap_or(i64::MAX),
            label,
            on_page_change: move |next_page| page.set(next_page),
        }
    }
}
