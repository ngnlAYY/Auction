//! Admin 结算与 recovery 页面装配。
//! 上方选择已结算场次的世界/期号，下方读取对应 UUID 的结算与 recovery 权威报告；
//! 所有屏幕均使用单列自然文档流，由页面统一滚动。

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

use super::pagination::use_admin_pagination;
use super::{AdminActionFeedback, admin_api_error_message, admin_world_label};
use crate::api::{
    AdminSettlementCatalogPage, AdminSettlementReport, fetch_admin_recovery,
    fetch_admin_settlement, fetch_admin_settlement_catalog,
};
use crate::surface::pending::{
    PendingCommandPanel, use_pending_command_gate, use_pending_commands_with_gate,
};

mod helpers;
mod picker;
mod query;
mod recovery;
mod report;

use picker::{AdminSettlementCatalogPager, AdminSettlementPicker};
use query::{
    SettlementQuery, read_settlement_query, replace_settlement_query,
    use_settlement_location_revision,
};
use recovery::AdminRecoveryDialog;
use report::AdminSettlementReportView;

#[derive(Debug, Clone, PartialEq)]
struct RecoveryConfirmationDraft {
    auction_id: String,
    recovery_id: String,
    state_version: i64,
    action: String,
    equipment_name: String,
    lot_code: String,
}

struct CatalogLoad {
    world_id: String,
    page: i64,
    page_size: i64,
    result: Result<AdminSettlementCatalogPage, String>,
}

struct CatalogLookupLoad {
    world_id: String,
    sequence: Option<i64>,
    result: Result<Option<AdminSettlementCatalogPage>, String>,
}

struct DetailLoad<T> {
    auction_id: String,
    result: Result<Option<T>, String>,
}

/// 加载并展示已结算拍卖的多条件报表页面。
pub(super) fn page() -> Element {
    let initial_query = read_settlement_query();
    let initial_world = initial_query.world_id;
    let initial_auction = initial_query.auction_id;
    let initial_sequence = initial_query.sequence;
    let initial_page = initial_query.page;
    let mut world_filter = use_signal(|| initial_world);
    let mut selected_auction_id = use_signal(|| initial_auction);
    let mut requested_sequence = use_signal(|| initial_sequence);
    let mut catalog_page = use_signal(|| initial_page);
    let catalog_pagination =
        use_admin_pagination("settlement-catalog", &["page_size"], Some(100), 100_000);
    let catalog_page_size = catalog_pagination.size_signal();
    let lot_page = use_signal(|| 1_i64);
    let recovery_page = use_signal(|| 1_i64);
    let reload_generation = use_signal(|| 0_u64);
    let recovery_reload_generation = use_signal(|| 0_u64);
    let confirmation = use_signal(|| None::<RecoveryConfirmationDraft>);
    let reason = use_signal(String::new);
    let mut action_busy = use_signal(|| false);
    let action_feedback = use_signal(|| None::<AdminActionFeedback>);
    let selection_generation = use_signal(|| 0_u64);
    let command_gate = use_pending_command_gate();
    let pending = use_pending_commands_with_gate(reload_generation, Some(command_gate));
    use_effect(move || action_busy.set(pending.any_busy()));
    let location_revision = use_settlement_location_revision();

    // 浏览器返回/前进可能只改变 query；按地址恢复筛选并关闭旧报告上的操作上下文。
    use_effect(move || {
        let _ = location_revision();
        let query = read_settlement_query();
        let unchanged = world_filter.read_unchecked().as_str() == query.world_id
            && selected_auction_id.read_unchecked().as_str() == query.auction_id
            && *requested_sequence.read_unchecked() == query.sequence
            && *catalog_page.read_unchecked() == query.page;
        if unchanged {
            return;
        }
        world_filter.set(query.world_id);
        selected_auction_id.set(query.auction_id);
        requested_sequence.set(query.sequence);
        catalog_page.set(query.page);
        reset_detail_state(
            lot_page,
            recovery_page,
            confirmation,
            reason,
            action_feedback,
            selection_generation,
        );
    });

    let catalog = use_resource(move || {
        let _ = reload_generation();
        let world_id = world_filter();
        let page = catalog_page();
        let page_size = catalog_page_size();
        async move {
            let result =
                fetch_admin_settlement_catalog(world_id.clone(), None, page, page_size).await;
            CatalogLoad { world_id, page, page_size, result }
        }
    });
    // URL 中的期号可能不在当前目录页；单独使用服务端筛选解析它，同时保留完整分页目录供切换。
    let catalog_lookup = use_resource(move || {
        let world_id = world_filter();
        let sequence = requested_sequence();
        async move {
            let result = match sequence {
                Some(sequence) => {
                    fetch_admin_settlement_catalog(world_id.clone(), Some(sequence), 1, 10)
                        .await
                        .map(Some)
                }
                None => Ok(None),
            };
            CatalogLookupLoad { world_id, sequence, result }
        }
    });
    let settlement = use_resource(move || {
        let _ = reload_generation();
        let auction_id = selected_auction_id();
        async move {
            let result = if auction_id.is_empty() {
                Ok(None)
            } else {
                fetch_admin_settlement(auction_id.clone()).await.map(Some)
            };
            DetailLoad { auction_id, result }
        }
    });
    let recovery = use_resource(move || {
        let _ = reload_generation();
        let _ = recovery_reload_generation();
        let auction_id = selected_auction_id();
        async move {
            let result = if auction_id.is_empty() {
                Ok(None)
            } else {
                fetch_admin_recovery(auction_id.clone()).await.map(Some)
            };
            DetailLoad { auction_id, result }
        }
    });

    // 每个筛选页都以服务端稳定排序的第一项为默认值；第一页因此始终选择最后结算的一期。
    use_effect(move || {
        let catalog_state = catalog.read();
        let Some(load) = catalog_state.as_ref() else {
            return;
        };
        let current_world = world_filter();
        let current_page = catalog_page();
        if load.world_id != current_world
            || load.page != current_page
            || load.page_size != catalog_page_size()
        {
            return;
        }
        let Ok(page) = &load.result else {
            return;
        };
        if page.auctions.is_empty() && page.total > 0 && current_page > 1 {
            let last_page =
                ((page.total - 1) / page.page_size + 1).max(1).min(100_000 / page.page_size + 1);
            catalog_page.set(last_page);
            selected_auction_id.set(String::new());
            requested_sequence.set(None);
            reset_detail_state(
                lot_page,
                recovery_page,
                confirmation,
                reason,
                action_feedback,
                selection_generation,
            );
            replace_settlement_query(&SettlementQuery {
                world_id: current_world,
                page: last_page,
                ..SettlementQuery::default()
            });
            return;
        }

        let current_id = selected_auction_id();
        let desired_sequence = requested_sequence();
        let selected_on_page = find_requested_entry(page, &current_id, desired_sequence).cloned();
        let selected_from_lookup = if selected_on_page.is_none() && desired_sequence.is_some() {
            let lookup_state = catalog_lookup.read();
            let Some(load) = lookup_state.as_ref() else {
                return;
            };
            if load.world_id != current_world || load.sequence != desired_sequence {
                return;
            }
            match &load.result {
                Ok(Some(page)) => {
                    find_requested_entry(page, &current_id, desired_sequence).cloned()
                }
                Ok(None) => None,
                // 解析失败时保留地址中的选择并展示错误，不能静默改成最新一期。
                Err(_) => return,
            }
        } else {
            None
        };
        let selected = selected_on_page.or(selected_from_lookup).or_else(|| {
            desired_sequence.is_none().then(|| page.auctions.first().cloned()).flatten()
        });
        // 带期号的地址必须解析为同一条目录事实；不存在或互相矛盾时保留地址并显式报错。
        if selected.is_none() && desired_sequence.is_some() {
            return;
        }
        let Some(selected) = selected else {
            if !current_id.is_empty() {
                selected_auction_id.set(String::new());
                requested_sequence.set(None);
                reset_detail_state(
                    lot_page,
                    recovery_page,
                    confirmation,
                    reason,
                    action_feedback,
                    selection_generation,
                );
            }
            replace_settlement_query(&SettlementQuery {
                world_id: current_world,
                page: current_page,
                ..SettlementQuery::default()
            });
            return;
        };
        if selected.auction_id != current_id {
            selected_auction_id.set(selected.auction_id.clone());
            reset_detail_state(
                lot_page,
                recovery_page,
                confirmation,
                reason,
                action_feedback,
                selection_generation,
            );
        }
        if desired_sequence != Some(selected.sequence) {
            requested_sequence.set(Some(selected.sequence));
        }
        replace_settlement_query(&SettlementQuery {
            world_id: current_world,
            auction_id: selected.auction_id,
            sequence: Some(selected.sequence),
            page: current_page,
        });
    });

    let current_world = world_filter();
    let current_catalog_page = catalog_page();
    let selected = selected_auction_id();
    let desired_sequence = requested_sequence();
    // 先借用完整快照核对身份，只在交给子组件时复制所需结果，避免重绘时复制两遍报告。
    let catalog_state = catalog.read_unchecked();
    let catalog_lookup_state = catalog_lookup.read_unchecked();
    let settlement_state = settlement.read_unchecked();
    let recovery_state = recovery.read_unchecked();
    let current_catalog = catalog_state.as_ref().filter(|load| {
        load.world_id == current_world
            && load.page == current_catalog_page
            && load.page_size == catalog_page_size()
    });
    let catalog_data = current_catalog.and_then(|load| load.result.as_ref().ok());
    let selected_on_page = catalog_data
        .as_ref()
        .and_then(|page| find_requested_entry(page, &selected, desired_sequence))
        .cloned();
    let lookup_needed = selected_on_page.is_none() && desired_sequence.is_some();
    let current_catalog_lookup = catalog_lookup_state
        .as_ref()
        .filter(|load| load.world_id == current_world && load.sequence == desired_sequence);
    let lookup_data =
        current_catalog_lookup.and_then(|load| load.result.as_ref().ok()).and_then(Option::as_ref);
    let selected_entry = selected_on_page.or_else(|| {
        lookup_data
            .and_then(|page| find_requested_entry(page, &selected, desired_sequence))
            .cloned()
    });
    let mut picker_catalog = catalog_data.cloned();
    if let (Some(page), Some(entry)) = (&mut picker_catalog, selected_entry.as_ref())
        && !page.auctions.iter().any(|candidate| candidate.auction_id == entry.auction_id)
    {
        // 深链接目标不在当前分页时仍把它放入选择器；翻页后恢复该页的普通目录。
        page.auctions.push(entry.clone());
    }
    let catalog_loading =
        current_catalog.is_none() || lookup_needed && current_catalog_lookup.is_none();
    let lookup_missing = lookup_needed
        && current_catalog_lookup.is_some_and(|load| match load.result.as_ref() {
            Ok(Some(page)) => find_requested_entry(page, &selected, desired_sequence).is_none(),
            Ok(None) => true,
            Err(_) => false,
        });
    let catalog_error = current_catalog
        .and_then(|load| load.result.as_ref().err())
        .or_else(|| {
            lookup_needed
                .then_some(current_catalog_lookup)
                .flatten()
                .and_then(|load| load.result.as_ref().err())
        })
        .map(|error| admin_api_error_message(error))
        .or_else(|| lookup_missing.then(|| admin_api_error_message("invalid_response")));

    let current_settlement = settlement_state.as_ref().filter(|load| load.auction_id == selected);
    let current_recovery = recovery_state.as_ref().filter(|load| load.auction_id == selected);
    let mut detail_error = current_settlement
        .and_then(|load| load.result.as_ref().err())
        .or_else(|| current_recovery.and_then(|load| load.result.as_ref().err()))
        .map(|error| admin_api_error_message(error));
    let report = current_settlement
        .and_then(|load| load.result.as_ref().ok())
        .and_then(Option::as_ref)
        .filter(|report| report_matches_selection(report, selected_entry.as_ref()))
        .cloned();
    if current_settlement
        .and_then(|load| load.result.as_ref().ok())
        .and_then(Option::as_ref)
        .is_some()
        && report.is_none()
    {
        detail_error = Some(admin_api_error_message("invalid_response"));
    }
    let recovery_report = current_recovery
        .and_then(|load| load.result.as_ref().ok())
        .and_then(Option::as_ref)
        .cloned();
    let detail_loading =
        !selected.is_empty() && (current_settlement.is_none() || current_recovery.is_none());
    let report_title = selected_entry.as_ref().map_or_else(
        || t!("admin-settlement-report-panel-title"),
        |entry| {
            t!(
                "admin-settlement-report-title",
                world: admin_world_label(&entry.world_id),
                sequence: entry.sequence
            )
        },
    );

    rsx! {
        section {
            class: "admin-auction-page admin-settlement-page",
            aria_labelledby: "settlement-title",
            h1 { id: "settlement-title", {t!("page-settlement-title")} }
            PendingCommandPanel { owner: pending }
            div { class: "admin-settlement-layout",
                section {
                    class: "admin-settlement-layout__selector",
                    aria_labelledby: "settlement-selector-title",
                    h2 { id: "settlement-selector-title", {t!("admin-settlement-selector-title")} }
                    p { class: "admin-auction-intro", {t!("admin-settlement-selector-note")} }
                    AdminSettlementPicker {
                        catalog: picker_catalog,
                        world_filter,
                        requested_sequence,
                        selected_auction_id,
                        catalog_page,
                        lot_page,
                        recovery_page,
                        confirmation,
                        reason,
                        action_feedback,
                        selection_generation,
                    }
                    if catalog_loading {
                        AsyncState {
                            kind: AsyncStateKind::Loading,
                            title: t!("admin-settlement-catalog-loading"),
                            detail: "",
                        }
                    }
                    if let Some(error) = catalog_error {
                        AsyncState {
                            kind: AsyncStateKind::Error,
                            title: error,
                            detail: "",
                        }
                    }
                    if let Some(page) = catalog_data {
                        AdminSettlementCatalogPager {
                            page: page.page,
                            page_size: page.page_size,
                            total: page.total,
                            pagination: catalog_pagination,
                            world_filter,
                            requested_sequence,
                            selected_auction_id,
                            catalog_page,
                            lot_page,
                            recovery_page,
                            confirmation,
                            reason,
                            action_feedback,
                            selection_generation,
                        }
                    }
                }
                section {
                    class: "admin-settlement-layout__report",
                    aria_labelledby: "settlement-report-panel-title",
                    h2 { id: "settlement-report-panel-title", "{report_title}" }
                    if selected.is_empty() {
                        p { class: "admin-auction-intro", {t!("admin-settlement-pick-prompt")} }
                    } else {
                        if detail_loading {
                            AsyncState {
                                kind: AsyncStateKind::Loading,
                                title: t!("admin-settlement-loading"),
                                detail: "",
                            }
                        }
                        if let Some(error) = detail_error {
                            AsyncState {
                                kind: AsyncStateKind::Error,
                                title: error,
                                detail: "",
                            }
                        }
                        if let Some(report) = report {
                            // key 必须位于迭代片段，才能随场次销毁报告内的分页草稿；页级命令宿主保留。
                            for (report, recovery) in [(report, recovery_report)] {
                                AdminSettlementReportView {
                                    key: "{report.auction.id}",
                                    report,
                                    recovery,
                                    lot_page,
                                    recovery_page,
                                    action_busy,
                                    confirmation,
                                    reason,
                                    action_feedback,
                                }
                            }
                        }
                    }
                    if let Some(feedback) = action_feedback() {
                        p {
                            class: if feedback.is_success() {
                                "admin-shell__inline-ok"
                            } else {
                                "admin-auction-error"
                            },
                            role: if feedback.is_success() { "status" } else { "alert" },
                            {feedback.message(t!("admin-recovery-action-success"))}
                        }
                    }
                }
            }
            AdminRecoveryDialog {
                draft: confirmation(),
                confirmation,
                reason,
                busy: action_busy,
                feedback: action_feedback,
                selected_auction_id,
                selection_generation,
                recovery_reload_generation,
            }
        }
    }
}

fn find_requested_entry<'a>(
    page: &'a AdminSettlementCatalogPage,
    auction_id: &str,
    sequence: Option<i64>,
) -> Option<&'a crate::api::AdminSettlementCatalogEntry> {
    if !auction_id.is_empty() {
        return page.auctions.iter().find(|entry| entry.auction_id == auction_id);
    }
    sequence.and_then(|sequence| page.auctions.iter().find(|entry| entry.sequence == sequence))
}

fn reset_detail_state(
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

fn report_matches_selection(
    report: &AdminSettlementReport,
    selected: Option<&crate::api::AdminSettlementCatalogEntry>,
) -> bool {
    selected.is_some_and(|selected| {
        report.auction.id == selected.auction_id
            && report.auction.world_id == selected.world_id
            && report.auction.sequence == selected.sequence
            && report.auction.status == "settled"
    })
}
