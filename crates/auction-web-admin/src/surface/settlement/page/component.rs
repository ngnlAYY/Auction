//! 结算页面组件：资源加载、选择状态和报告视图协调。

use auction_web_shared::{AsyncState, AsyncStateKind, QueryWriteMode, t};
use dioxus::prelude::*;

use super::super::picker::{AdminSettlementCatalogPager, AdminSettlementPicker};
use super::super::query::{
    SETTLEMENT_PAGE_SIZE, SettlementLocation, SettlementQuery, read_settlement_query,
    use_settlement_location,
};
use super::super::read_retry::SettlementRead;
use super::super::reads::{SettlementReadQuery, matching_lookup, use_settlement_reads};
use super::super::recovery::{
    AdminRecoveryDialog, RecoveryConfirmationDraft, RecoveryRead, use_recovery_focus_fallback,
};
use super::super::report::AdminSettlementReportView;
use super::super::selection::{CatalogSelection, report_for_selection, resolve_catalog_selection};
use crate::api::{AdminSettlementCatalogEntry, AdminSettlementCatalogPage};
use crate::surface::pagination::use_admin_pagination;
use crate::surface::pending::{
    PendingCommandPanel, use_pending_command_gate, use_pending_commands_with_gate,
};
use crate::surface::{AdminActionFeedback, admin_api_error_message, admin_world_label};

#[component]
pub(super) fn AdminSettlementPage(
    initial_query: SettlementQuery,
    location_revision: Signal<u64>,
    reload_generation: Signal<u64>,
) -> Element {
    let location = use_settlement_location(location_revision, &initial_query);
    let applied_location_revision = location.applied_revision;
    let initial_world = initial_query.world_id;
    let initial_auction = initial_query.auction_id;
    let initial_sequence = initial_query.sequence;
    let initial_page = initial_query.page;
    let mut world_filter = use_signal(|| initial_world);
    let mut selected_auction_id = use_signal(|| initial_auction);
    let mut requested_sequence = use_signal(|| initial_sequence);
    let mut catalog_page = use_signal(|| initial_page);
    let catalog_pagination =
        use_admin_pagination("settlement-catalog", &[SETTLEMENT_PAGE_SIZE], Some(100), 100_000);
    let catalog_page_size = catalog_pagination.size_signal();
    let lot_page = use_signal(|| 1_i64);
    let recovery_page = use_signal(|| 1_i64);
    let recovery_reload_generation = use_signal(|| 0_u64);
    let confirmation = use_signal(|| None::<RecoveryConfirmationDraft>);
    let reason = use_signal(String::new);
    let action_feedback = use_signal(|| None::<AdminActionFeedback>);
    let selection_generation = use_signal(|| 0_u64);

    // 同页导航及前进后退按地址恢复筛选，选择变化时关闭旧报告上的操作上下文。
    use_effect(move || {
        let revision = location_revision();
        if revision == *applied_location_revision.peek() {
            return;
        }
        let Ok(query) = read_settlement_query() else {
            return;
        };
        let unchanged = world_filter.read_unchecked().as_str() == query.world_id
            && selected_auction_id.read_unchecked().as_str() == query.auction_id
            && *requested_sequence.read_unchecked() == query.sequence
            && *catalog_page.read_unchecked() == query.page;
        if !unchanged {
            world_filter.set(query.world_id.clone());
            selected_auction_id.set(query.auction_id.clone());
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
        }
        location.apply(revision, query);
    });

    let reads = use_settlement_reads(
        SettlementReadQuery {
            world: world_filter,
            page: catalog_page,
            page_size: catalog_page_size,
            auction_id: selected_auction_id,
            sequence: requested_sequence,
        },
        reload_generation,
        recovery_reload_generation,
    );
    let catalog = reads.catalog;
    let catalog_lookup = reads.lookup;
    let recovery = reads.recovery;
    let command_gate = use_pending_command_gate();
    let pending =
        use_pending_commands_with_gate(reload_generation, Some(command_gate), move |entity| {
            let Some(recovery_id) = entity.strip_prefix("settlement-recovery:") else {
                return false;
            };
            let selected = selected_auction_id.peek();
            !selected.is_empty()
                && recovery
                    .peek()
                    .as_ref()
                    .map(|(_, load)| load)
                    .filter(|load| load.auction_id == *selected)
                    .and_then(|load| load.result.as_ref().ok())
                    .and_then(Option::as_ref)
                    .is_none_or(|report| {
                        report.recoveries.iter().any(|recovery| recovery.id == recovery_id)
                    })
        });
    // 派生值供恢复报告和确认框订阅，避免命令忙碌时重绘整页。
    let action_busy = use_memo(move || pending.any_busy());

    // 每个筛选页都以服务端稳定排序的第一项为默认值；第一页因此始终选择最后结算的一期。
    use_effect(move || {
        // 先恢复新位置，再允许匹配本地筛选的目录响应更新场次和地址。
        if location_revision() != applied_location_revision() {
            return;
        }
        let catalog_state = catalog.read();
        let Some(load) = catalog_state.as_ref() else {
            return;
        };
        let current_world = world_filter();
        let current_page = catalog_page();
        if load.world_id != current_world
            || load.page != current_page
            || load.page_size != catalog_page_size()
            || !location.matches_address()
        {
            return;
        }
        let Ok(page) = &load.result else {
            return;
        };
        let current_id = selected_auction_id();
        let desired_sequence = requested_sequence();
        if reconcile_empty_catalog_tail(
            page,
            location,
            catalog_page,
            &current_world,
            &current_id,
            desired_sequence,
        ) {
            return;
        }

        let lookup_state = catalog_lookup.read();
        let lookup = matching_lookup(lookup_state.as_ref(), &current_world, desired_sequence)
            .and_then(|load| load.result.as_ref().ok())
            .and_then(Option::as_ref);
        let CatalogSelection::Matched(selected) =
            resolve_catalog_selection(Some(page), lookup, &current_id, desired_sequence)
        else {
            // 深链接未解析或已证实矛盾均保留地址，不能静默改为最新一期。
            return;
        };
        let selected = selected.clone();
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
        location.write(
            SettlementQuery {
                world_id: current_world,
                auction_id: selected.auction_id,
                sequence: Some(selected.sequence),
                page: current_page,
            },
            QueryWriteMode::Replace,
        );
    });

    let current_world = world_filter();
    let current_catalog_page = catalog_page();
    let catalog_location_revision = applied_location_revision();
    let selected = selected_auction_id();
    let desired_sequence = requested_sequence();
    // 先借用完整快照核对身份，只在交给子组件时复制所需结果，避免重绘时复制两遍报告。
    let catalog_state = catalog.read_unchecked();
    let catalog_lookup_state = catalog_lookup.read_unchecked();
    let settlement_state = reads.settlement.read_unchecked();
    let recovery_state = recovery.read_unchecked();
    let current_catalog = catalog_state.as_ref().filter(|load| {
        load.world_id == current_world
            && load.page == current_catalog_page
            && load.page_size == catalog_page_size()
    });
    let catalog_data = current_catalog.and_then(|load| load.result.as_ref().ok());
    let lookup_needed = desired_sequence.is_some()
        && catalog_data.is_some()
        && matches!(
            resolve_catalog_selection(catalog_data, None, &selected, desired_sequence),
            CatalogSelection::Resolving
        );
    let current_catalog_lookup =
        matching_lookup(catalog_lookup_state.as_ref(), &current_world, desired_sequence);
    let lookup_data =
        current_catalog_lookup.and_then(|load| load.result.as_ref().ok()).and_then(Option::as_ref);
    let selection =
        resolve_catalog_selection(catalog_data, lookup_data, &selected, desired_sequence);
    let selected_entry = match selection {
        CatalogSelection::Matched(entry) => Some(entry),
        CatalogSelection::Resolving | CatalogSelection::Contradictory => None,
    };
    let picker_catalog = catalog_for_picker(catalog_data, selected_entry);
    // 重读期间保留同查询的错误，按钮仍对应原读取。
    let catalog_error = current_catalog.and_then(|load| load.result.as_ref().err());
    let lookup_error = current_catalog_lookup
        .filter(|_| lookup_needed)
        .and_then(|load| load.result.as_ref().err());
    let catalog_loading = current_catalog.is_none()
        || catalog_error.is_some() && reads.pending(SettlementRead::Catalog)
        || lookup_needed && current_catalog_lookup.is_none()
        || lookup_error.is_some() && reads.pending(SettlementRead::CatalogLookup);

    let current_settlement = settlement_state.as_ref().filter(|load| load.auction_id == selected);
    // 恢复表单依赖恢复报告的版本；会话宿主收到回执后，旧报告不能授权新命令。
    let recovery_generation = (reload_generation(), recovery_reload_generation());
    let recovery_read = RecoveryRead {
        reload: reload_generation,
        recovery_reload: recovery_reload_generation,
        generation: recovery_generation,
    };
    let current_recovery = recovery_state
        .as_ref()
        .filter(|(loaded, load)| *loaded == recovery_generation && load.auction_id == selected)
        .map(|(_, load)| load);
    let candidate_report =
        current_settlement.and_then(|load| load.result.as_ref().ok()).and_then(Option::as_ref);
    let checked_report = report_for_selection(selection, candidate_report);
    let settlement_error = current_settlement
        .and_then(|load| load.result.as_ref().err())
        .map(String::as_str)
        .or_else(|| checked_report.as_ref().err().copied());
    let report = checked_report.ok().flatten().cloned();
    let recovery_error = current_recovery.and_then(|load| load.result.as_ref().err());
    let recovery_report = report
        .as_ref()
        .and(current_recovery)
        .and_then(|load| load.result.as_ref().ok())
        .and_then(Option::as_ref)
        .cloned();
    // 确认框和命令入口都必须由目录与详情的同一条场次事实授权。
    let recovery_draft = confirmation().filter(|draft| {
        recovery_report.as_ref().is_some_and(|report| {
            report.recoveries.iter().any(|entry| {
                entry.id == draft.recovery_id && entry.state_version == draft.state_version
            })
        })
    });
    use_recovery_focus_fallback(recovery_draft.is_some());
    let settlement_loading = current_settlement.is_none()
        || settlement_error.is_some() && reads.pending(SettlementRead::Settlement);
    let recovery_loading = current_recovery.is_none()
        || recovery_error.is_some() && reads.pending(SettlementRead::Recovery);
    let report_title = selected_entry.map_or_else(
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
                    h2 { id: "settlement-selector-title", tabindex: "-1", {t!("admin-settlement-selector-title")} }
                    p { class: "admin-auction-intro", {t!("admin-settlement-selector-note")} }
                    AdminSettlementPicker {
                        catalog: picker_catalog,
                        location,
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
                    if let Some(code) = catalog_error {
                        {reads.error(SettlementRead::Catalog, code)}
                    }
                    if let Some(code) = lookup_error {
                        {reads.error(SettlementRead::CatalogLookup, code)}
                    }
                    if matches!(selection, CatalogSelection::Contradictory) {
                        AsyncState {
                            kind: AsyncStateKind::Error,
                            title: admin_api_error_message("invalid_response"),
                            detail: "",
                        }
                    }
                    // 空尾页只由核对了位置与响应身份的目录归并纠偏，保留原场次链接。
                    if let Some(page) = catalog_data.filter(|page| !is_empty_catalog_tail(page)) {
                        AdminSettlementCatalogPager {
                            page: page.page,
                            page_size: page.page_size,
                            total: page.total,
                            world_id: current_world,
                            location,
                            catalog_revision: catalog_location_revision,
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
                    h2 { id: "settlement-report-panel-title", tabindex: "-1", "{report_title}" }
                    if selected.is_empty() {
                        p { class: "admin-auction-intro", {t!("admin-settlement-pick-prompt")} }
                    } else if selected_entry.is_some() {
                        if settlement_loading {
                            AsyncState {
                                kind: AsyncStateKind::Loading,
                                title: t!("admin-settlement-loading"),
                                detail: "",
                            }
                        }
                        if let Some(code) = settlement_error {
                            {reads.error(SettlementRead::Settlement, code)}
                        }
                        // 随场次重建报告，但同场次的恢复标题不随结算报告的读取结果卸载。
                        for (auction_id, report, recovery) in [(selected, report, recovery_report)] {
                            AdminSettlementReportView {
                                key: "{auction_id}",
                                report,
                                recovery,
                                lot_page,
                                recovery_page,
                                action_busy,
                                recovery_read,
                                confirmation,
                                reason,
                                action_feedback,
                                recovery_status: rsx! {
                                    if recovery_loading {
                                        AsyncState {
                                            kind: AsyncStateKind::Loading,
                                            title: t!("admin-settlement-loading"),
                                            detail: "",
                                        }
                                    }
                                    if let Some(code) = recovery_error {
                                        {reads.error(SettlementRead::Recovery, code)}
                                    }
                                },
                            }
                        }
                    }
                    if let Some(feedback) = action_feedback() {
                        p {
                            class: if feedback.is_success() {
                                "action-feedback action-feedback--success"
                            } else {
                                "action-feedback action-feedback--error"
                            },
                            role: if feedback.is_success() { "status" } else { "alert" },
                            {feedback.message(t!("admin-recovery-action-success"))}
                        }
                    }
                }
            }
            if let Some(draft) = recovery_draft {
                AdminRecoveryDialog {
                    draft,
                    location,
                    confirmation,
                    reason,
                    busy: action_busy,
                    feedback: action_feedback,
                    selected_auction_id,
                    selection_generation,
                    recovery_read,
                    recovery_reload_generation,
                }
            }
        }
    }
}

fn is_empty_catalog_tail(page: &AdminSettlementCatalogPage) -> bool {
    page.auctions.is_empty() && page.total > 0 && page.page > 1
}

/// 空尾页归并只改页码，保留待解析的场次与期号。
pub(super) fn reconcile_empty_catalog_tail(
    page: &AdminSettlementCatalogPage,
    location: SettlementLocation,
    mut catalog_page: Signal<i64>,
    world: &str,
    auction: &str,
    sequence: Option<i64>,
) -> bool {
    if !is_empty_catalog_tail(page) {
        return false;
    }
    let last_page =
        ((page.total - 1) / page.page_size + 1).max(1).min(100_000 / page.page_size + 1);
    catalog_page.set(last_page);
    location.write(
        SettlementQuery {
            world_id: world.to_owned(),
            auction_id: auction.to_owned(),
            sequence,
            page: last_page,
        },
        QueryWriteMode::Replace,
    );
    true
}

/// 深链接目标不在当前分页时，将补查出的场次放入选择器。
pub(super) fn catalog_for_picker(
    page: Option<&AdminSettlementCatalogPage>,
    selected: Option<&AdminSettlementCatalogEntry>,
) -> Option<AdminSettlementCatalogPage> {
    let mut catalog = page.cloned();
    if let (Some(page), Some(entry)) = (&mut catalog, selected)
        && !page.auctions.iter().any(|candidate| candidate.auction_id == entry.auction_id)
    {
        page.auctions.push(entry.to_owned());
    }
    catalog
}

/// 切换场次时复位分页与确认草稿，推进世代以隔离旧场次的回执。
pub(in crate::surface::settlement) fn reset_detail_state(
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
