//! 结算页面的地址恢复、目录纠偏和报告核验回归测试。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use auction_web_shared::QueryWriteMode;
use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
use dioxus::dioxus_core::Mutation;
use dioxus::prelude::{
    ReadableExt, VNode, VirtualDom, dioxus_core, dioxus_signals, rsx, use_context_provider,
    use_effect, use_resource, use_signal,
};
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

use super::super::picker::AdminSettlementPicker;
use super::super::query::{
    TestSettlementSearch, read_settlement_query, use_settlement_location,
    use_settlement_location_revision,
};
use super::super::selection::{CatalogSelection, report_for_selection, resolve_catalog_selection};
use super::super::tests::{CatalogReads, TestLocation, use_test_location};
use super::component::{catalog_for_picker, reconcile_empty_catalog_tail};
use super::page;
use crate::api::{AdminSettlementCatalogEntry, AdminSettlementCatalogPage, AdminSettlementReport};
use crate::surface::ADMIN_LOCALE_CATALOG;
use crate::surface::auctions::editor::tests::{RecordRead, init_i18n, mount_writer, settle_writer};
use crate::surface::realtime::{get_parameter, selected_options, text_attributes};

const AUCTION_ID: &str = "00000000-0000-0000-0000-000000000001";
const LINK_AUCTION: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

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
        crate::surface::pending::use_pending_command_host();
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
fn empty_catalog_tail_replaces_page_once_and_preserves_deep_link() {
    let address = Rc::new(TestLocation::at(&format!(
        "/settlement?world=main&page=2&page_size=10&auction={LINK_AUCTION}&sequence=7"
    )));
    let reply = Rc::new(RecordRead::<AdminSettlementCatalogPage>::new());
    let current_page = Rc::new(Cell::new(None));
    let requested_pages = Rc::new(RefCell::new(Vec::new()));
    let (mut page, _) = mount_writer({
        let address = address.clone();
        let reply = reply.clone();
        let current_page = current_page.clone();
        let requested_pages = requested_pages.clone();
        move || {
            use_test_location(address.clone());
            let query = read_settlement_query().unwrap();
            let location = use_settlement_location(use_settlement_location_revision(), &query);
            let catalog_page = use_signal(|| query.page);
            current_page.set(Some(catalog_page));
            let reply = reply.clone();
            let requested_pages = requested_pages.clone();
            let catalog = use_resource(move || {
                let page = catalog_page();
                let reply = reply.clone();
                let requested_pages = requested_pages.clone();
                async move {
                    requested_pages.borrow_mut().push(page);
                    reply.fetch().await
                }
            });
            use_effect(move || {
                if let Some(Ok(page)) = catalog.read().as_ref()
                    && page.page == catalog_page()
                {
                    reconcile_empty_catalog_tail(
                        page,
                        location,
                        catalog_page,
                        &query.world_id,
                        &query.auction_id,
                        query.sequence,
                    );
                }
            });
            VNode::empty()
        }
    });
    assert!(address.attempts.borrow().is_empty());
    assert_eq!(requested_pages.borrow().as_slice(), [2]);
    reply.respond(Ok(AdminSettlementCatalogPage { page: 2, auctions: vec![], ..catalog() }));
    settle_writer(&mut page);
    assert_eq!(*current_page.get().unwrap().peek(), 1, "目录页码应归并到末页");
    assert_eq!(requested_pages.borrow().as_slice(), [2, 1]);
    reply.respond(Ok(catalog()));
    settle_writer(&mut page);
    assert_eq!(requested_pages.borrow().as_slice(), [2, 1]);
    let attempts = address.attempts.borrow();
    let [(mode, url)] = attempts.as_slice() else {
        panic!("空尾页应恰好写入一次历史：{attempts:?}");
    };
    assert_eq!(*mode, QueryWriteMode::Replace);
    assert_eq!(get_parameter(url, "page"), None);
    assert_eq!(get_parameter(url, "page_size").as_deref(), Some("10"));
    assert_eq!(get_parameter(url, "world").as_deref(), Some("main"));
    assert_eq!(get_parameter(url, "auction").as_deref(), Some(LINK_AUCTION));
    assert_eq!(get_parameter(url, "sequence").as_deref(), Some("7"));
}

#[test]
fn settlement_page_reads_corrected_catalog_page_after_empty_tail() {
    let address =
        Rc::new(TestLocation::at("/settlement?world=main&page=2&page_size=10&sequence=7"));
    let catalog_reads = Rc::new(CatalogReads::new());
    let (mut mounted, _) = mount_writer({
        let address = address.clone();
        let catalog_reads = catalog_reads.clone();
        move || {
            use_test_location(address.clone());
            use_context_provider(|| catalog_reads.clone());
            page()
        }
    });
    assert_eq!(catalog_reads.pages.borrow().as_slice(), [2]);
    // 分页器在挂载时会写入每页数量，只记录回包后的历史修改。
    address.attempts.borrow_mut().clear();
    catalog_reads.reply.respond(Ok(AdminSettlementCatalogPage {
        page: 2,
        auctions: vec![],
        ..catalog()
    }));
    settle_writer(&mut mounted);

    let attempts = address.attempts.borrow();
    let [(mode, url)] = attempts.as_slice() else {
        panic!("空尾页应恰好写入一次历史：{attempts:?}");
    };
    assert_eq!(*mode, QueryWriteMode::Replace);
    assert_eq!(get_parameter(url, "page"), None);
    assert_eq!(get_parameter(url, "page_size").as_deref(), Some("10"));
    assert_eq!(get_parameter(url, "world").as_deref(), Some("main"));
    assert_eq!(get_parameter(url, "sequence").as_deref(), Some("7"));
    assert_eq!(catalog_reads.pages.borrow().as_slice(), [2, 1]);
}

#[test]
fn late_catalog_lookup_inserts_and_selects_the_deep_link_option() {
    const OTHER: &str = "00000000-0000-4000-8000-000000000002";
    let address = Rc::new(TestLocation::at(&format!(
        "/settlement?world=main&auction={LINK_AUCTION}&sequence=7"
    )));
    let reply = Rc::new(RecordRead::<AdminSettlementCatalogPage>::new());
    let (mut page, edits) = mount_writer({
        let reply = reply.clone();
        move || {
            init_i18n();
            use_test_location(address.clone());
            let query = read_settlement_query().unwrap();
            let location = use_settlement_location(use_settlement_location_revision(), &query);
            let world_filter = use_signal(|| query.world_id);
            let requested_sequence = use_signal(|| query.sequence);
            let selected_auction_id = use_signal(|| query.auction_id);
            let catalog_page = use_signal(|| query.page);
            let lot_page = use_signal(|| 1);
            let recovery_page = use_signal(|| 1);
            let confirmation = use_signal(|| None);
            let reason = use_signal(String::new);
            let action_feedback = use_signal(|| None);
            let selection_generation = use_signal(|| 0);
            let reply = reply.clone();
            let lookup = use_resource(move || {
                let reply = reply.clone();
                async move { reply.fetch().await }
            });
            let mut page = catalog();
            page.auctions[0].auction_id = OTHER.to_owned();
            page.auctions[0].sequence = 8;
            let lookup = lookup.read();
            let selected = match resolve_catalog_selection(
                Some(&page),
                lookup.as_ref().and_then(|reply| reply.as_ref().ok()),
                &selected_auction_id.read(),
                requested_sequence(),
            ) {
                CatalogSelection::Matched(entry) => Some(entry),
                CatalogSelection::Resolving | CatalogSelection::Contradictory => None,
            };
            let catalog = catalog_for_picker(Some(&page), selected);
            rsx! {
                AdminSettlementPicker {
                    catalog, location, world_filter, requested_sequence, selected_auction_id,
                    catalog_page, lot_page, recovery_page, confirmation, reason,
                    action_feedback, selection_generation,
                }
            }
        }
    });
    assert!(text_attributes(&edits, "value").any(|(_, value)| value == OTHER));
    assert!(selected_options(&edits).is_empty());
    let mut lookup = catalog();
    lookup.auctions[0].auction_id = LINK_AUCTION.to_owned();
    reply.respond(Ok(lookup));
    let edits = settle_writer(&mut page);
    assert_eq!(selected_options(&edits), [LINK_AUCTION]);
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
