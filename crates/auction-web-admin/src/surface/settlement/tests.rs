//! 宿主结算地址、详情交付围栏和目录分页事件的回归测试。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use auction_web_shared::QueryWriteMode;
use auction_web_shared::query::{QueryLocation, use_query_router};
use dioxus::dioxus_core::Mutation;
use dioxus::history::MemoryHistory;
use dioxus::prelude::{
    Element, History, ReadableExt, Signal, VirtualDom, WritableExt, component, consume_context,
    dioxus_core, dioxus_signals, provide_context, rsx, use_effect, use_signal,
};
use gloo_net::http::Method;

use super::picker::AdminSettlementCatalogPager;
use super::query::{
    SettlementQuery, read_settlement_query, use_settlement_location,
    use_settlement_location_revision,
};
use super::recovery::{AdminRecoveryDialog, RecoveryConfirmationDraft, RecoveryRead};
use crate::api::{AdminApiFailure, AdminSettlementCatalogPage};
use crate::surface::auctions::editor::tests::{
    RecordRead, button, init_i18n, mount_writer, settle_writer, writer_form,
};
use crate::surface::pagination::use_admin_pagination;
use crate::surface::pending::{PendingCommandPanel, use_pending_commands};
use crate::surface::realtime::text_attributes;
use crate::surface::test_events::dispatch_form_event;

const AUCTION: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const BOOKMARK: &str = "/settlement?world=main&page=2&page_size=10&auction=0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b&sequence=5";

pub(super) struct TestLocation {
    history: Rc<dyn History>,
    tick: Cell<Option<Signal<u64>>>,
    pub(super) attempts: RefCell<Vec<(QueryWriteMode, String)>>,
    reject: Cell<bool>,
}

impl TestLocation {
    fn new() -> Self {
        Self::at(BOOKMARK)
    }

    pub(super) fn at(url: &str) -> Self {
        Self {
            history: Rc::new(MemoryHistory::with_initial_path(url)),
            tick: Cell::new(None),
            attempts: RefCell::new(Vec::new()),
            reject: Cell::new(false),
        }
    }

    fn notify(&self) {
        let mut tick = self.tick.get().unwrap();
        let next = *tick.peek() + 1;
        tick.set(next);
    }

    fn navigate(&self, url: &str) {
        self.history.push(url.to_owned());
        self.notify();
    }
}

impl QueryLocation for TestLocation {
    fn current(&self) -> String {
        self.history.current_route()
    }

    fn write(&self, url: &str, mode: QueryWriteMode) -> bool {
        self.attempts.borrow_mut().push((mode, url.to_owned()));
        if self.reject.get() {
            return false;
        }
        match mode {
            QueryWriteMode::Push => self.history.push(url.to_owned()),
            QueryWriteMode::Replace => self.history.replace(url.to_owned()),
        }
        self.notify();
        true
    }
}

#[derive(Default)]
struct Observations {
    restored: RefCell<Vec<SettlementQuery>>,
    write: Cell<Option<dioxus::prelude::Callback<SettlementQuery>>>,
    deliver: Cell<Option<dioxus::prelude::Callback<(), bool>>>,
    catalog_page: Cell<Option<Signal<i64>>>,
    lot_page: Cell<Option<Signal<i64>>>,
    recovery_page: Cell<Option<Signal<i64>>>,
    auction: Cell<Option<Signal<String>>>,
}

pub(super) fn use_test_location(location: Rc<TestLocation>) {
    let tick = use_signal(|| 0_u64);
    location.tick.set(Some(tick));
    provide_context(location.history.clone());
    provide_context::<Rc<dyn QueryLocation>>(location.clone());
    use_query_router(move || {
        let _version = tick();
        location.current()
    });
}

pub(super) struct CatalogReads {
    pub(super) pages: RefCell<Vec<i64>>,
    pub(super) reply: RecordRead<AdminSettlementCatalogPage>,
}

impl CatalogReads {
    pub(super) fn new() -> Self {
        Self { pages: RefCell::new(Vec::new()), reply: RecordRead::new() }
    }
}

/// 宿主夹具只替换目录请求的传输，页码订阅与归并仍由真实页面处理。
pub(super) async fn fetch_admin_settlement_catalog(
    world: String,
    sequence: Option<i64>,
    page: i64,
    page_size: i64,
) -> Result<AdminSettlementCatalogPage, String> {
    if let Some(catalog) = dioxus::prelude::try_consume_context::<Rc<CatalogReads>>() {
        if sequence.is_some() {
            return std::future::pending().await;
        }
        assert_eq!(world, "main");
        assert_eq!(page_size, 10);
        catalog.pages.borrow_mut().push(page);
        return catalog.reply.fetch().await;
    }
    crate::api::fetch_admin_settlement_catalog(world, sequence, page, page_size).await
}

fn root() -> Element {
    init_i18n();
    use_test_location(consume_context::<Rc<TestLocation>>());
    rsx! { Directory {} }
}

#[component]
fn Directory() -> Element {
    let observed = consume_context::<Rc<Observations>>();
    let revision = use_settlement_location_revision();
    let initial = read_settlement_query().unwrap();
    let location = use_settlement_location(revision, &initial);
    let world = use_signal(|| initial.world_id);
    let sequence = use_signal(|| initial.sequence);
    let auction = use_signal(|| initial.auction_id);
    let catalog_page = use_signal(|| initial.page);
    let lot_page = use_signal(|| 3);
    let recovery_page = use_signal(|| 4);
    let confirmation = use_signal(|| None);
    let reason = use_signal(String::new);
    let feedback = use_signal(|| None);
    let generation = use_signal(|| 0);
    let pagination = use_admin_pagination("settlement-catalog", &["page_size"], Some(100), 100_000);
    observed.write.set(Some(dioxus::prelude::Callback::new(move |query| {
        location.write(query, QueryWriteMode::Replace);
    })));
    observed
        .deliver
        .set(Some(dioxus::prelude::Callback::new(move |()| location.can_deliver_receipt())));
    observed.catalog_page.set(Some(catalog_page));
    observed.lot_page.set(Some(lot_page));
    observed.recovery_page.set(Some(recovery_page));
    observed.auction.set(Some(auction));
    use_effect(move || {
        let revision = revision();
        if let Ok(query) = read_settlement_query() {
            observed.restored.borrow_mut().push(query.clone());
            location.apply(revision, query);
        }
    });
    rsx! {
        AdminSettlementCatalogPager {
            page: catalog_page(), page_size: pagination.size(), total: 100,
            world_id: world(), location, catalog_revision: (location.applied_revision)(),
            pagination, world_filter: world, requested_sequence: sequence,
            selected_auction_id: auction, catalog_page, lot_page, recovery_page,
            confirmation, reason, action_feedback: feedback, selection_generation: generation,
        }
    }
}

fn render(dom: &mut VirtualDom) -> Vec<Mutation> {
    (0..4).flat_map(|_| dom.render_immediate_to_vec().edits).collect()
}

fn mount() -> (VirtualDom, Rc<TestLocation>, Rc<Observations>, Vec<Mutation>) {
    let address = Rc::new(TestLocation::new());
    let observed = Rc::new(Observations::default());
    let mut dom = VirtualDom::new(root)
        .with_root_context(address.clone())
        .with_root_context(observed.clone());
    let mut edits = dom.rebuild_to_vec().edits;
    edits.extend(render(&mut dom));
    (dom, address, observed, edits)
}

#[test]
fn same_page_navigation_restores_only_changed_query_and_fences_old_receipts() {
    let (mut dom, address, observed, _) = mount();
    let deliver = observed.deliver.get().unwrap();
    assert!(deliver.call(()));
    assert_eq!(observed.restored.borrow().len(), 1);
    let normalized = address.current();
    address.notify();
    render(&mut dom);
    address.navigate(&format!("{normalized}#report"));
    render(&mut dom);
    assert_eq!(observed.restored.borrow().len(), 1);

    address.navigate("/settlement");
    assert!(!deliver.call(()), "地址变化应立即阻止旧详情领取回执");
    render(&mut dom);
    assert!(deliver.call(()));
    assert_eq!(observed.restored.borrow()[1], SettlementQuery::default());

    address.history.replace(format!("/settlement?auction={AUCTION}&sequence=5&world=main"));
    address.notify();
    render(&mut dom);
    assert_eq!(observed.restored.borrow()[2].auction_id, AUCTION);
    address.history.go_back();
    address.notify();
    render(&mut dom);
    assert_eq!(observed.restored.borrow()[3].page, 2);
    address.history.go_forward();
    address.notify();
    render(&mut dom);
    assert_eq!(observed.restored.borrow()[4].sequence, Some(5));

    observed.write.get().unwrap().call(SettlementQuery::default());
    render(&mut dom);
    assert_eq!(observed.restored.borrow().len(), 5);
    address.navigate("/dashboard");
    render(&mut dom);
    assert_eq!(observed.restored.borrow().len(), 5);
}

#[test]
fn refused_write_keeps_current_receipt_target_until_address_changes() {
    let (_dom, address, observed, _) = mount();
    let deliver = observed.deliver.get().unwrap();
    address.reject.set(true);
    observed.write.get().unwrap().call(SettlementQuery::default());
    assert!(deliver.call(()));
    assert_eq!(address.attempts.borrow().last().unwrap().0, QueryWriteMode::Replace);
    address.navigate(&BOOKMARK.replace("page_size=10", "page_size=20"));
    assert!(!deliver.call(()), "曾拒写时，只变页长也不能向旧详情交付");
}

#[test]
fn recovery_conflict_does_not_reload_old_report_after_address_changes() {
    const RECOVERY: &str = "00000000-0000-4000-8000-000000000001";
    const PATH: &str = "/api/v1/admin/recoveries/00000000-0000-4000-8000-000000000001/confirm";
    for navigate in [false, true] {
        let address = Rc::new(TestLocation::new());
        let reload_observed = Rc::new(Cell::new(None));
        let (mut page, edits) = mount_writer({
            let address = address.clone();
            let reload_observed = reload_observed.clone();
            move || {
                use_test_location(address.clone());
                AdminApiFailure::stub_write_response(PATH, Method::POST, 409, "{");
                let revision = use_settlement_location_revision();
                let location = use_settlement_location(revision, &read_settlement_query().unwrap());
                let reload = use_signal(|| 0);
                let recovery_reload = use_signal(|| 0);
                reload_observed.set(Some(recovery_reload));
                let owner = use_pending_commands(reload, |_| false);
                let confirmation = use_signal(|| {
                    Some(RecoveryConfirmationDraft {
                        auction_id: AUCTION.to_owned(),
                        recovery_id: RECOVERY.to_owned(),
                        state_version: 2,
                        action: "reclaimed".to_owned(),
                        equipment_name: "Phase Robe".to_owned(),
                        lot_code: "C1".to_owned(),
                    })
                });
                let reason = use_signal(String::new);
                let feedback = use_signal(|| None);
                let selected = use_signal(|| AUCTION.to_owned());
                let generation = use_signal(|| 0);
                let busy = use_signal(|| false);
                rsx! {
                    PendingCommandPanel { owner }
                    if let Some(draft) = confirmation() {
                        AdminRecoveryDialog {
                            draft, location, confirmation, reason, busy, feedback,
                            selected_auction_id: selected, selection_generation: generation,
                            recovery_read: RecoveryRead { reload, recovery_reload, generation: (0, 0) },
                            recovery_reload_generation: recovery_reload,
                        }
                    }
                }
            }
        });
        let event = dispatch_form_event(page.dom(), writer_form(&edits), "submit", "");
        assert!(!event.default_action_enabled());
        if navigate {
            // 地址栏先变，旧页面仍持有原场次与世代，收包时须独立核对地址。
            address.navigate("/settlement?world=isekai");
        }
        settle_writer(&mut page);
        assert!(button(&page, "原样重试").is_some());
        assert_eq!(*reload_observed.get().unwrap().peek(), u64::from(!navigate));
    }
}

#[test]
fn stale_page_size_events_never_write_and_current_event_only_resets_directory() {
    for custom in [false, true] {
        let (mut dom, address, observed, edits) = mount();
        let select = text_attributes(&edits, "name")
            .find(|(_, value)| *value == "page_size")
            .map(|(id, _)| id)
            .or_else(|| {
                edits.iter().find_map(|edit| match edit {
                    Mutation::NewEventListener { name, id } if name == "change" => Some(*id),
                    _ => None,
                })
            })
            .expect("每页条数选择器");
        let target = if custom {
            let input = text_attributes(&edits, "name")
                .find(|(_, value)| *value == "custom_page_size")
                .map(|(id, _)| id)
                .or_else(|| {
                    edits.iter().rev().find_map(|edit| match edit {
                        Mutation::NewEventListener { name, id } if name == "input" => Some(*id),
                        _ => None,
                    })
                })
                .expect("自定义条数输入框");
            let apply = text_attributes(&edits, "class")
                .find(|(_, value)| *value == "ui-pager__custom")
                .map(|(id, _)| id)
                .or_else(|| {
                    edits.iter().rev().find_map(|edit| match edit {
                        Mutation::NewEventListener { name, id } if name == "submit" => Some(*id),
                        _ => None,
                    })
                })
                .expect("应用页长表单");
            dispatch_form_event(&dom, input, "input", "30");
            render(&mut dom);
            apply
        } else {
            select
        };
        let change = || {
            if custom {
                dispatch_form_event(&dom, target, "submit", "");
            } else {
                dispatch_form_event(&dom, target, "change", "20");
            }
        };
        address.attempts.borrow_mut().clear();
        address.navigate("/settlement?world=isekai&page_size=10");
        change();
        assert!(address.attempts.borrow().is_empty());
        assert_eq!(address.current(), "/settlement?world=isekai&page_size=10");
        assert_eq!(*observed.catalog_page.get().unwrap().peek(), 2);

        address.history.go_back();
        address.notify();
        change();
        assert_eq!(*observed.catalog_page.get().unwrap().peek(), 1);
        assert_eq!(*observed.lot_page.get().unwrap().peek(), 3);
        assert_eq!(*observed.recovery_page.get().unwrap().peek(), 4);
        assert_eq!(observed.auction.get().unwrap().peek().as_str(), AUCTION);
        let attempts = address.attempts.borrow();
        assert_eq!(
            attempts.iter().map(|(mode, _)| *mode).collect::<Vec<_>>(),
            [QueryWriteMode::Replace, QueryWriteMode::Push, QueryWriteMode::Replace]
        );
        assert!(address.current().contains(if custom { "page_size=30" } else { "page_size=20" }));
        assert!(!address.current().contains("&page=2"));
    }
}
