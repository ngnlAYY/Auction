//! 结算与恢复页面：目录、详情加载与恢复确认。

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::picker::{AdminSettlementCatalogPager, AdminSettlementPicker};
use super::query::{
    SETTLEMENT_PAGE_SIZE, SettlementQuery, read_settlement_query, replace_settlement_query,
    use_settlement_location_revision,
};
use super::recovery::AdminRecoveryDialog;
use super::report::AdminSettlementReportView;
use crate::api::{
    AdminSettlementCatalogEntry, AdminSettlementCatalogPage, AdminSettlementReport,
    fetch_admin_recovery, fetch_admin_settlement, fetch_admin_settlement_catalog,
};
use crate::surface::pagination::use_admin_pagination;
use crate::surface::pending::{
    PendingCommandPanel, PendingCommands, use_pending_command_gate, use_pending_commands_with_gate,
};
use crate::surface::{AdminActionFeedback, admin_api_error_message, admin_world_label};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RecoveryConfirmationDraft {
    pub(super) auction_id: String,
    pub(super) recovery_id: String,
    pub(super) state_version: i64,
    pub(super) action: String,
    pub(super) equipment_name: String,
    pub(super) lot_code: String,
}

pub(super) struct CatalogLoad {
    pub(super) world_id: String,
    pub(super) page: i64,
    pub(super) page_size: i64,
    pub(super) result: Result<AdminSettlementCatalogPage, String>,
}

pub(super) struct CatalogLookupLoad {
    pub(super) world_id: String,
    pub(super) sequence: Option<i64>,
    pub(super) result: Result<Option<AdminSettlementCatalogPage>, String>,
}

pub(super) struct DetailLoad<T> {
    pub(super) auction_id: String,
    pub(super) result: Result<Option<T>, String>,
}

/// 加载并展示已结算拍卖的多条件报表页面。
pub(crate) fn page() -> Element {
    let location_revision = use_settlement_location_revision();
    let _ = location_revision();
    let reload_generation = use_signal(|| 0_u64);
    let command_gate = use_pending_command_gate();
    // 同路径切到无效链接时只卸载读取与编辑视图，已经提交的命令仍归页级宿主持有。
    let pending = use_pending_commands_with_gate(reload_generation, Some(command_gate));
    match read_settlement_query() {
        Ok(initial_query) => rsx! {
            AdminSettlementPage { initial_query, location_revision, reload_generation, pending }
        },
        Err(_invalid_link) => rsx! {
            section {
                class: "admin-auction-page admin-settlement-page",
                aria_labelledby: "settlement-title",
                h1 { id: "settlement-title", {t!("page-settlement-title")} }
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("admin-settlement-invalid-link"),
                    detail: "",
                }
            }
        },
    }
}

#[component]
fn AdminSettlementPage(
    initial_query: SettlementQuery,
    location_revision: Signal<u64>,
    reload_generation: Signal<u64>,
    pending: PendingCommands,
) -> Element {
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
    // 派生值：只通知读取它的报告与确认框，页面本身不因命令忙碌状态多渲染一轮。
    let action_busy = use_memo(move || pending.any_busy());

    // 浏览器返回/前进可能只改变 query；按地址恢复筛选并关闭旧报告上的操作上下文。
    use_effect(move || {
        let _ = location_revision();
        let Ok(query) = read_settlement_query() else {
            return;
        };
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
    // 只有当前目录页已加载且页内找不到请求的期号时才查找：默认选中与下拉选择的期号都在页内，
    // 不再多发一次结果用不到的请求；深链接目标不在当前页时，查找在目录页返回后发出。
    let lookup_sequence = use_memo(move || {
        let sequence = requested_sequence()?;
        let catalog_state = catalog.read();
        let load = catalog_state.as_ref()?;
        let current_page = load.world_id == *world_filter.read()
            && load.page == catalog_page()
            && load.page_size == catalog_page_size();
        let page = load.result.as_ref().ok().filter(|_| current_page)?;
        let selected = selected_auction_id.read();
        matches!(
            resolve_catalog_selection(Some(page), None, &selected, Some(sequence)),
            CatalogSelection::Resolving
        )
        .then_some(sequence)
    });
    let catalog_lookup = use_resource(move || {
        let world_id = world_filter();
        let sequence = lookup_sequence();
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
        let current_id = selected_auction_id();
        let desired_sequence = requested_sequence();
        if page.auctions.is_empty() && page.total > 0 && current_page > 1 {
            let last_page =
                ((page.total - 1) / page.page_size + 1).max(1).min(100_000 / page.page_size + 1);
            catalog_page.set(last_page);
            // 钳制空页只改页码；仍须解析原深链接的 UUID 与期号。
            replace_settlement_query(&SettlementQuery {
                world_id: current_world,
                auction_id: current_id,
                sequence: desired_sequence,
                page: last_page,
            });
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
    let mut picker_catalog = catalog_data.cloned();
    if let (Some(page), Some(entry)) = (&mut picker_catalog, selected_entry)
        && !page.auctions.iter().any(|candidate| candidate.auction_id == entry.auction_id)
    {
        // 深链接目标不在当前分页时仍把它放入选择器；翻页后恢复该页的普通目录。
        page.auctions.push(entry.to_owned());
    }
    let catalog_loading =
        current_catalog.is_none() || lookup_needed && current_catalog_lookup.is_none();
    let catalog_error = current_catalog
        .and_then(|load| load.result.as_ref().err())
        .or_else(|| {
            lookup_needed
                .then_some(current_catalog_lookup)
                .flatten()
                .and_then(|load| load.result.as_ref().err())
        })
        .map(|error| admin_api_error_message(error))
        .or_else(|| {
            matches!(selection, CatalogSelection::Contradictory)
                .then(|| admin_api_error_message("invalid_response"))
        });

    let current_settlement = settlement_state.as_ref().filter(|load| load.auction_id == selected);
    let current_recovery = recovery_state.as_ref().filter(|load| load.auction_id == selected);
    let mut detail_error = selected_entry.and_then(|_| {
        current_settlement
            .and_then(|load| load.result.as_ref().err())
            .or_else(|| current_recovery.and_then(|load| load.result.as_ref().err()))
            .map(|error| admin_api_error_message(error))
    });
    let candidate_report =
        current_settlement.and_then(|load| load.result.as_ref().ok()).and_then(Option::as_ref);
    let report = match report_for_selection(selection, candidate_report) {
        Ok(report) => report.cloned(),
        Err(code) => {
            detail_error = Some(admin_api_error_message(code));
            None
        }
    };
    let recovery_report = report
        .as_ref()
        .and(current_recovery)
        .and_then(|load| load.result.as_ref().ok())
        .and_then(Option::as_ref)
        .cloned();
    // 确认框和命令入口都必须由目录与详情的同一条场次事实授权。
    let recovery_draft = recovery_report.as_ref().and_then(|_| confirmation());
    let detail_loading = selected_entry.is_some()
        && !selected.is_empty()
        && (current_settlement.is_none() || current_recovery.is_none());
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogSelection<'a> {
    Resolving,
    Matched(&'a AdminSettlementCatalogEntry),
    Contradictory,
}

fn matching_lookup<'a>(
    load: Option<&'a CatalogLookupLoad>,
    world: &str,
    sequence: Option<i64>,
) -> Option<&'a CatalogLookupLoad> {
    load.filter(|load| load.world_id == world && load.sequence == sequence)
}

fn resolve_catalog_selection<'a>(
    page: Option<&'a AdminSettlementCatalogPage>,
    lookup: Option<&'a AdminSettlementCatalogPage>,
    auction_id: &str,
    sequence: Option<i64>,
) -> CatalogSelection<'a> {
    let Some(page) = page else {
        return CatalogSelection::Resolving;
    };
    let Some(sequence) = sequence else {
        return if auction_id.is_empty() {
            page.auctions.first().map_or(CatalogSelection::Resolving, CatalogSelection::Matched)
        } else {
            CatalogSelection::Resolving
        };
    };

    // 有 UUID 时只能解析到该记录。不同世界可以复用同一个期号，不能取查询的第一项代替。
    if let Some(entry) = page.auctions.iter().find(|entry| {
        if auction_id.is_empty() {
            entry.sequence == sequence
        } else {
            entry.auction_id == auction_id
        }
    }) {
        return if entry.sequence == sequence {
            CatalogSelection::Matched(entry)
        } else {
            CatalogSelection::Contradictory
        };
    }
    lookup.map_or(CatalogSelection::Resolving, |lookup| {
        lookup
            .auctions
            .iter()
            .find(|entry| {
                entry.sequence == sequence
                    && (auction_id.is_empty() || entry.auction_id == auction_id)
            })
            .map_or(CatalogSelection::Contradictory, CatalogSelection::Matched)
    })
}

pub(super) fn reset_detail_state(
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

fn report_for_selection<'a>(
    selection: CatalogSelection<'_>,
    report: Option<&'a AdminSettlementReport>,
) -> Result<Option<&'a AdminSettlementReport>, &'static str> {
    let (CatalogSelection::Matched(entry), Some(report)) = (selection, report) else {
        return Ok(None);
    };
    if report.auction.id != entry.auction_id
        || report.auction.world_id != entry.world_id
        || report.auction.sequence != entry.sequence
        || report.auction.status != "settled"
    {
        return Err("invalid_response");
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::dioxus_core::Mutation;
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::super::query::TestSettlementSearch;
    use super::{CatalogSelection, page, report_for_selection, resolve_catalog_selection};
    use crate::api::{
        AdminSettlementCatalogEntry, AdminSettlementCatalogPage, AdminSettlementReport,
    };
    use crate::surface::ADMIN_LOCALE_CATALOG;

    const AUCTION_ID: &str = "00000000-0000-0000-0000-000000000001";

    #[test]
    fn duplicate_page_size_mounts_only_the_invalid_link_view() {
        let search = url::form_urlencoded::parse(b"sequence=7&page_size=10&page_size=20");
        let mut dom = VirtualDom::new(|| {
            try_use_init_i18n_from_bundle(|| {
                build_bundle(
                    &DEFAULT_ADMIN_LOCALE,
                    ADMIN_LOCALE_CATALOG.default_ftl,
                    &DEFAULT_ADMIN_LOCALE,
                    None,
                )
            })
            .unwrap();
            page()
        })
        .with_root_context(Rc::new(TestSettlementSearch(search.into_owned().collect())));
        let mut edits = dom.rebuild_to_vec().edits;
        edits.extend(dom.render_immediate_to_vec().edits);
        assert!(edits.iter().any(|edit| matches!(edit,
            Mutation::CreateTextNode { value, .. } if value.contains("此结算链接的参数无效")
        )));
        // 目录、恢复入口和会改写地址的分页器都不应挂载。
        assert!(!edits.iter().any(|edit| matches!(edit, Mutation::NewEventListener { .. })));
    }

    fn catalog() -> AdminSettlementCatalogPage {
        AdminSettlementCatalogPage {
            auctions: vec![AdminSettlementCatalogEntry {
                auction_id: AUCTION_ID.to_owned(),
                world_id: "main".to_owned(),
                sequence: 7,
                settled_at: "2026-11-14T09:05:00Z".to_owned(),
            }],
            page: 1,
            page_size: 10,
            total: 1,
        }
    }

    #[test]
    fn deep_link_requires_both_fields_to_match() {
        let page = catalog();
        assert_eq!(
            resolve_catalog_selection(Some(&page), None, AUCTION_ID, Some(7)),
            CatalogSelection::Matched(&page.auctions[0])
        );
        assert_eq!(
            resolve_catalog_selection(Some(&page), None, AUCTION_ID, Some(8)),
            CatalogSelection::Contradictory
        );
    }

    #[test]
    fn a_matching_lookup_resolves_only_after_the_directory_loads() {
        let lookup = catalog();
        let other_id = "00000000-0000-0000-0000-000000000002";
        let mut page = catalog();
        page.auctions[0].auction_id = other_id.to_owned();
        page.auctions[0].world_id = "isekai".to_owned();
        assert_eq!(
            resolve_catalog_selection(None, Some(&lookup), AUCTION_ID, Some(7)),
            CatalogSelection::Resolving
        );
        assert_eq!(
            resolve_catalog_selection(Some(&page), None, AUCTION_ID, Some(7)),
            CatalogSelection::Resolving
        );
        assert_eq!(
            resolve_catalog_selection(Some(&page), Some(&lookup), AUCTION_ID, Some(7)),
            CatalogSelection::Matched(&lookup.auctions[0])
        );
        assert_eq!(
            resolve_catalog_selection(Some(&page), Some(&page), AUCTION_ID, Some(7)),
            CatalogSelection::Contradictory
        );
    }

    #[test]
    fn a_report_arriving_first_can_wait_without_an_invalid_response() {
        let page = catalog();
        let selected = CatalogSelection::Matched(&page.auctions[0]);
        let mut report: AdminSettlementReport = serde_json::from_value(serde_json::json!({
            "auction": {
                "id": AUCTION_ID, "world_id": "main", "sequence": 7, "status": "settled", "state_version": 2,
                "preview_scheduled_at": "2026-11-13T09:00:00Z", "scheduled_open_at": "2026-11-14T09:00:00Z",
                "preview_manual_open": false, "preview_revision": 0,
                "created_at": "2026-11-13T08:00:00Z", "updated_at": "2026-11-14T09:05:00Z"
            },
            "settlement": null, "lots": []
        })).unwrap();
        assert_eq!(report_for_selection(CatalogSelection::Resolving, Some(&report)), Ok(None));
        assert_eq!(report_for_selection(selected, Some(&report)), Ok(Some(&report)));
        report.auction.sequence = 8;
        assert_eq!(report_for_selection(selected, Some(&report)), Err("invalid_response"));
    }
}
