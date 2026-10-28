//! Admin 结算报表的世界、期号选择器与目录分页。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{AdminSettlementCatalogEntry, AdminSettlementCatalogPage};
use crate::app::surface::{AdminActionFeedback, format_admin_time};

use crate::app::surface::pagination::{AdminPager as Pager, AdminPagination};

use super::RecoveryConfirmationDraft;
use super::query::{SettlementQuery, push_settlement_query};

#[component]
pub(super) fn AdminSettlementPicker(
    catalog: Option<AdminSettlementCatalogPage>,
    mut world_filter: Signal<String>,
    mut requested_sequence: Signal<Option<i64>>,
    mut selected_auction_id: Signal<String>,
    mut catalog_page: Signal<i64>,
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<AdminActionFeedback>>,
    mut selection_generation: Signal<u64>,
) -> Element {
    let entries = catalog.as_ref().map(|page| page.auctions.clone()).unwrap_or_default();
    let selection_entries = entries.clone();
    let period_disabled = entries.is_empty();

    rsx! {
        div { class: "admin-settlement-selector__fields",
            label { r#for: "settlement-world",
                {t!("admin-settlement-select-world")}
                select {
                    id: "settlement-world",
                    value: world_filter(),
                    onchange: move |event| {
                        let next_world = event.value();
                        if !matches!(next_world.as_str(), "all" | "main" | "isekai") {
                            return;
                        }
                        world_filter.set(next_world.clone());
                        requested_sequence.set(None);
                        selected_auction_id.set(String::new());
                        catalog_page.set(1);
                        reset_report_state(
                            lot_page,
                            recovery_page,
                            confirmation,
                            reason,
                            action_feedback,
                            selection_generation,
                        );
                        push_settlement_query(&SettlementQuery {
                            world_id: next_world,
                            ..SettlementQuery::default()
                        });
                    },
                    option { value: "all", {t!("admin-settlement-world-all")} }
                    option { value: "main", {t!("admin-runtime-world-main")} }
                    option { value: "isekai", {t!("admin-runtime-world-isekai")} }
                }
            }
            label { r#for: "settlement-period",
                {t!("admin-settlement-select-period")}
                select {
                    id: "settlement-period",
                    disabled: period_disabled,
                    value: selected_auction_id(),
                    onchange: move |event| {
                        let auction_id = event.value();
                        let selected = selection_entries
                            .iter()
                            .find(|entry| entry.auction_id == auction_id)
                            .cloned();
                        let Some(selected) = selected else {
                            return;
                        };
                        selected_auction_id.set(selected.auction_id.clone());
                        requested_sequence.set(Some(selected.sequence));
                        reset_report_state(
                            lot_page,
                            recovery_page,
                            confirmation,
                            reason,
                            action_feedback,
                            selection_generation,
                        );
                        push_settlement_query(&SettlementQuery {
                            world_id: world_filter(),
                            auction_id: selected.auction_id,
                            sequence: Some(selected.sequence),
                            page: catalog_page(),
                        });
                    },
                    if entries.is_empty() {
                        option { value: "", {t!("admin-settlement-no-periods")} }
                    } else {
                        for entry in entries {
                            option {
                                value: entry.auction_id.clone(),
                                {period_option_label(&entry)}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn AdminSettlementCatalogPager(
    page: i64,
    page_size: i64,
    total: i64,
    pagination: AdminPagination,
    world_filter: Signal<String>,
    mut requested_sequence: Signal<Option<i64>>,
    mut selected_auction_id: Signal<String>,
    mut catalog_page: Signal<i64>,
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<AdminActionFeedback>>,
    mut selection_generation: Signal<u64>,
) -> Element {
    rsx! {
        Pager {
            pagination,
            page: page.max(1),
            page_size: page_size.max(1),
            total: Some(total),
            label: t!("admin-settlement-auction-pagination"),
            previous_label: t!("admin-pagination-previous"),
            status: move |(page, pages): (i64, i64)| {
                t!("admin-pagination-status", page: page, pages: pages)
            },
            next_label: t!("admin-pagination-next"),
            jump_label: t!("admin-pagination-input"),
            jump_button_label: t!("admin-pagination-go"),
            on_size_reset: move |_| {
                // 数量只重置目录页；保持选定场次、两个报告分页及未结束的命令上下文。
                catalog_page.set(1);
                push_settlement_query(&SettlementQuery {
                    world_id: world_filter(),
                    auction_id: selected_auction_id(),
                    sequence: requested_sequence(),
                    page: 1,
                });
            },
            on_page_change: move |next_page| {
                catalog_page.set(next_page);
                requested_sequence.set(None);
                selected_auction_id.set(String::new());
                reset_report_state(
                    lot_page,
                    recovery_page,
                    confirmation,
                    reason,
                    action_feedback,
                    selection_generation,
                );
                push_settlement_query(&SettlementQuery {
                    world_id: world_filter(),
                    page: next_page,
                    ..SettlementQuery::default()
                });
            },
        }
    }
}

fn reset_report_state(
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<AdminActionFeedback>>,
    mut selection_generation: Signal<u64>,
) {
    lot_page.set(1);
    recovery_page.set(1);
    confirmation.set(None);
    reason.set(String::new());
    action_feedback.set(None);
    selection_generation.set(selection_generation().saturating_add(1));
}

fn period_option_label(entry: &AdminSettlementCatalogEntry) -> String {
    t!(
        "admin-settlement-period-option",
        world: world_label(&entry.world_id),
        sequence: entry.sequence,
        settled_at: format_admin_time(&entry.settled_at)
    )
}

fn world_label(world_id: &str) -> String {
    match world_id {
        "main" => t!("admin-runtime-world-main"),
        "isekai" => t!("admin-runtime-world-isekai"),
        _ => world_id.to_owned(),
    }
}
