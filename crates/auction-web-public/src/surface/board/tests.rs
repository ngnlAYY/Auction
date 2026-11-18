//! 驱动真实看板与资源，使用受控替身记录浏览器请求和竞价定时器。

use std::cell::{Cell, RefCell};
use std::future::{Future, pending, poll_fn};
use std::rc::Rc;
use std::task::{Poll, Waker};

use auction_time::Timestamp;
use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
use auction_web_shared::query::QueryLocation;
use auction_web_shared::{PageVisibility, QueryWriteMode, use_second_ticker};
use dioxus::dioxus_core::{AttributeValue, Mutation, Mutations, NoOpMutations};
use dioxus::history::MemoryHistory;
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

use super::refresh::PublicBidRefresh;
use super::world::tests::CatalogTimerFixture;
use super::{
    AuctionIndex, AuctionPage, PublicDisplayedAuction, PublicPageRealtimeTarget,
    PublicRealtimeOwner, PublicRealtimeTarget,
};
use crate::api::{
    AuctionListPage, CurrentAuction, HistoricalAuction, LotEnvelope, LotItems, PublicSession,
    PublicSessionCheckTrigger, ReplayFixture, SessionRefreshReason, TransportError,
};
use crate::surface::commands::session_fixture;
use crate::surface::router::{PUBLIC_LOCALE_CATALOG, Route, provide_test_public_history};
use crate::surface::watches::SkipWatchReads;
use crate::surface::{
    CurrentAuctionResource, LiveConnectionState, LotViewRequest, PageReadFixture, PublicBoardQuery,
    PublicCommandOwner, PublicContentLoading, PublicReadinessResource, PublicSessionResource,
    dispatch_page_event, page_test_contexts, rendered_event_target, run_page_events, settle_page,
};

const AUCTION: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000001";
const ALICE: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000002";
const BOB: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000003";
const OPENS_AT: &str = "2026-11-13T00:00:00Z";
const PREVIEW_OPENS: &str = "2099-11-20T00:00:00Z";

#[derive(Default)]
struct Gate {
    opened: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl Gate {
    async fn wait(&self) {
        poll_fn(|context| {
            if self.opened.get() {
                Poll::Ready(())
            } else {
                *self.waker.borrow_mut() = Some(context.waker().clone());
                Poll::Pending
            }
        })
        .await;
    }

    fn open(&self) {
        self.opened.set(true);
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

struct Request {
    gate: Rc<Gate>,
    auction_id: String,
    category: Option<String>,
    page: i64,
}

#[derive(Clone, Copy)]
struct Controls {
    auction: Signal<(String, Result<Option<CurrentAuction>, String>)>,
    session: Signal<Option<Option<PublicSession>>>,
    owner: Signal<Option<String>>,
    refresh: Signal<u64>,
    bids: PublicBidRefresh,
}

#[derive(Default)]
struct Script {
    initial_user: Option<&'static str>,
    initial_auction: Option<CurrentAuction>,
    catalog_status: Option<&'static str>,
    catalog_revision: i64,
    controls: Option<Controls>,
    query: Option<Signal<PublicBoardQuery>>,
    requests: Vec<Request>,
    timers: Vec<Rc<Gate>>,
}

thread_local! {
    static SCRIPT: RefCell<Option<Script>> = const { RefCell::new(None) };
}

pub(super) fn observe_query(query: Signal<PublicBoardQuery>) {
    SCRIPT.with_borrow_mut(|script| {
        if let Some(script) = script {
            script.query = Some(query);
        }
    });
}

pub(super) fn lots_request(
    auction_id: &str,
    category: Option<&str>,
    page: i64,
    page_size: i64,
) -> Option<impl Future<Output = Result<LotEnvelope, String>>> {
    let (gate, status, revision) = SCRIPT.with_borrow_mut(|script| {
        let script = script.as_mut()?;
        let gate = Rc::new(Gate::default());
        script.requests.push(Request {
            gate: Rc::clone(&gate),
            auction_id: auction_id.to_owned(),
            category: category.map(str::to_owned),
            page,
        });
        Some((gate, script.catalog_status.unwrap_or("live"), script.catalog_revision))
    })?;
    let auction_id = auction_id.to_owned();
    Some(async move {
        gate.wait().await;
        Ok(catalog_envelope(&auction_id, status, revision, page, page_size))
    })
}

fn catalog_envelope(
    auction_id: &str,
    status: &str,
    revision: i64,
    page: i64,
    page_size: i64,
) -> LotEnvelope {
    let preview = status == "preview";
    let mode = if preview { "preview" } else { "live" };
    LotEnvelope {
        auction_id: auction_id.to_owned(),
        auction_status: status.to_owned(),
        projection_kind: mode.to_owned(),
        presentation_mode: mode.to_owned(),
        bidding_open: status == "live",
        opens_at: (if preview { PREVIEW_OPENS } else { OPENS_AT }).to_owned(),
        preview_revision: revision,
        final_restriction: None,
        lots: preview.then(|| {
            LotItems::Preview(vec![auction_contracts::PublicPreviewLotResponse {
                id: ALICE.to_owned(),
                auction_id: auction_id.to_owned(),
                code: "Shd01".to_owned(),
                status: "pending".to_owned(),
                seller_name: "Seller".to_owned(),
                start_price: 1_000,
                equipment_name: format!("Preview revision {revision}"),
                equipment_group: "rare".to_owned(),
                equipment_category: Some("Shield".to_owned()),
                equipment_level: None,
                primary_attribute_initials: None,
                avg_roll: None,
                game_equipment_url: String::new(),
            }])
        }),
        lot: None,
        total: Some(200),
        page: Some(page),
        page_size: Some(page_size),
        category_counts: None,
    }
}

pub(super) fn bid_timer() -> Option<impl Future<Output = ()>> {
    let gate = SCRIPT.with_borrow_mut(|script| {
        let script = script.as_mut()?;
        let gate = Rc::new(Gate::default());
        script.timers.push(Rc::clone(&gate));
        Some(gate)
    })?;
    Some(async move { gate.wait().await })
}

#[derive(Clone, PartialEq, Routable)]
enum TestRoute {
    #[route("/")]
    AuctionIndex {},
    #[route("/auction")]
    AuctionPage {},
}

#[component]
fn TestApp() -> Element {
    try_use_init_i18n_from_bundle(|| {
        build_bundle(
            &DEFAULT_PUBLIC_LOCALE,
            PUBLIC_LOCALE_CATALOG.default_ftl,
            &DEFAULT_PUBLIC_LOCALE,
            None,
        )
    })
    .expect("加载公开端英文译文");
    use_second_ticker();
    use_context_provider(PageVisibility::new);
    use_context_provider(|| SkipWatchReads);
    let readiness: PublicReadinessResource = use_resource(|| async { Ok(true) });
    use_context_provider(|| readiness);
    let initial_user = SCRIPT.with_borrow(|script| script.as_ref().unwrap().initial_user);
    let session_input = use_signal(|| initial_user.map(|user| Some(player(user))));
    let session: PublicSessionResource = use_resource(move || {
        let latest = session_input();
        async move {
            match latest {
                Some(snapshot) => Ok(snapshot),
                None => pending().await,
            }
        }
    });
    use_context_provider(|| session);
    let mut owner = use_signal(|| initial_user.map(str::to_owned));
    use_context_provider(|| PublicCommandOwner(owner));
    use_effect(move || {
        if let Some(Ok(snapshot)) = session.read().as_ref() {
            let next = snapshot.as_ref().map(|session| session.user_id.clone());
            if *owner.peek() != next {
                owner.set(next);
            }
        }
    });
    let auction_input = use_signal(|| {
        let auction = SCRIPT
            .with_borrow(|script| script.as_ref().unwrap().initial_auction.clone())
            .unwrap_or_else(|| active_auction("live"));
        ("main".to_owned(), Ok(Some(auction)))
    });
    let auction: CurrentAuctionResource = use_resource(move || {
        let current = auction_input();
        async move { current }
    });
    use_context_provider(|| auction);
    use_context_provider(|| Signal::new("main".to_owned()));
    use_context_provider(|| LotViewRequest(Signal::new(None)));
    use_context_provider(|| PublicDisplayedAuction(Signal::new(None)));
    use_context_provider(|| PublicPageRealtimeTarget(Signal::new(None)));
    use_context_provider(|| PublicContentLoading(Signal::new(Default::default())));
    let refresh = use_context_provider(|| Signal::new(0_u64));
    let bids = use_context_provider(PublicBidRefresh::new);
    SCRIPT.with_borrow_mut(|script| {
        script.as_mut().unwrap().controls =
            Some(Controls { auction: auction_input, session: session_input, owner, refresh, bids });
    });
    rsx! { Router::<TestRoute> {} }
}

fn active_auction(mode: &str) -> CurrentAuction {
    CurrentAuction {
        id: AUCTION.to_owned(),
        world: "main".to_owned(),
        number: 2,
        status: mode.to_owned(),
        opens_at: (if mode == "preview" { PREVIEW_OPENS } else { OPENS_AT }).to_owned(),
        scheduled_ends_at: None,
        presentation_mode: mode.to_owned(),
        bidding_open: mode == "live",
        preview_revision: 0,
        proxy_default_mode: Some("fast".to_owned()),
    }
}

fn historical_page(auction_id: &str, world: &str) -> AuctionListPage {
    AuctionListPage {
        auctions: vec![HistoricalAuction {
            id: auction_id.to_owned(),
            world_id: world.to_owned(),
            sequence: 1,
            status: "settled".to_owned(),
            scheduled_open_at: OPENS_AT.to_owned(),
            opened_at: Some(OPENS_AT.to_owned()),
            final_phase_at: None,
            effective_end_at: None,
            settled_at: Some(OPENS_AT.to_owned()),
            lot_count: 0,
            sold_count: 0,
            unsold_count: 0,
            sold_amount: 0,
            state_version: 1,
        }],
        page: Some(1),
        page_size: 200,
        total: Some(1),
        cursor: None,
        next_cursor: None,
        has_more: None,
    }
}

fn settle(dom: &mut VirtualDom) {
    for _ in 0..24 {
        dom.process_events();
        dom.render_immediate(&mut NoOpMutations);
    }
}

fn mounted(initial_user: Option<&'static str>) -> VirtualDom {
    SCRIPT.with_borrow_mut(|script| *script = Some(Script { initial_user, ..Script::default() }));
    let mut dom = VirtualDom::new(TestApp);
    dom.rebuild_in_place();
    settle(&mut dom);
    assert_eq!(started(), 1, "场次就绪即发起首个 lots 请求");
    dom
}

fn controls() -> Controls {
    SCRIPT.with_borrow(|script| script.as_ref().unwrap().controls.unwrap())
}

fn started() -> usize {
    SCRIPT.with_borrow(|script| script.as_ref().unwrap().requests.len())
}

struct InvalidBoardLocation(&'static str);

impl QueryLocation for InvalidBoardLocation {
    fn current(&self) -> String {
        format!("{}?auction=&auction=broken&world=isekai", self.0)
    }

    fn write(&self, _location: &str, _mode: QueryWriteMode) -> bool {
        panic!("非法场次不能触发地址纠偏")
    }
}

#[test]
fn invalid_auction_link_renders_a_current_auction_link_without_reading_lots() {
    for path in ["/", "/auction"] {
        SCRIPT.with_borrow_mut(|script| {
            *script = Some(Script { initial_user: Some(ALICE), ..Script::default() });
        });
        let mut dom = VirtualDom::new(TestApp);
        let address = Rc::new(InvalidBoardLocation(path));
        let history = Rc::new(MemoryHistory::with_initial_path(path));
        dom.provide_root_context(history.clone() as Rc<dyn History>);
        dom.provide_root_context(address as Rc<dyn QueryLocation>);
        let mut mutations = Mutations::default();
        dom.rebuild(&mut mutations);
        settle(&mut dom);
        assert_eq!(history.current_route(), path, "无效别名也必须保留地址");
        assert_eq!(started(), 0, "即使当前场次在竞价也不能读取目录");
        assert!(mutations.edits.iter().any(|edit| matches!(edit,
            Mutation::CreateTextNode { value, .. } if value == "The auction in this link is invalid."
        )));
        assert!(mutations.edits.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { name: "href", value: AttributeValue::Text(value), .. } if value == "/?world=isekai"
        )));
    }
}

fn public_board_shell() -> Element {
    page_test_contexts();
    session_fixture::use_session_fixture(ALICE);
    rsx! { Router::<Route> {} }
}

fn shell_fixture(enabled_world: &'static str) -> VirtualDom {
    session_fixture::reset();
    session_fixture::script([session_fixture::read(None)]);
    VirtualDom::new(public_board_shell)
        .with_root_context(Rc::new(CatalogTimerFixture::new(enabled_world, Timestamp::now())))
        .with_root_context(PageReadFixture::new(|()| Ok::<_, String>(true)))
}

#[test]
fn the_shell_publishes_a_historical_world_only_after_its_address_is_written() {
    run_page_events(
        module_path!(),
        "the_shell_publishes_a_historical_world_only_after_its_address_is_written",
        || {
            let parts = format!("auction={AUCTION}&%74ype=%63lo&&keep=%2b%20x&keep=#lots");
            let initial = format!("/?world=main&{parts}");
            let expected = format!("/?{}&world=isekai#lots", parts.strip_suffix("#lots").unwrap());
            let history = Rc::new(MemoryHistory::with_initial_path(&initial));
            let allowed = Rc::new(Cell::new(false));
            let writes = Rc::new(RefCell::new(Vec::new()));
            let requests = Rc::new(RefCell::new(Vec::new()));
            let lot_reads = PageReadFixture::new({
                let history = history.clone();
                let requests = requests.clone();
                move |url: String| {
                    requests.borrow_mut().push((url, history.current_route()));
                    Ok::<_, String>(catalog_envelope(AUCTION, "settled", 0, 1, 20))
                }
            });
            let mut dom = shell_fixture("main")
                .with_root_context(PageReadFixture::new(|_: String| {
                    Ok::<_, String>(None::<CurrentAuction>)
                }))
                .with_root_context(PageReadFixture::new(|_: String| {
                    Ok::<_, String>(historical_page(AUCTION, "isekai"))
                }))
                .with_root_context(lot_reads);
            provide_test_public_history::<Route>(&mut dom, history.clone(), {
                let allowed = allowed.clone();
                let writes = writes.clone();
                move |url, mode| {
                    if url.contains("world=isekai") {
                        assert_eq!(mode, QueryWriteMode::Replace);
                        writes.borrow_mut().push(url.to_owned());
                        return allowed.get();
                    }
                    true
                }
            });
            let mut edits = dom.rebuild_to_vec().edits;
            edits.extend(settle_page(&mut dom));
            assert_eq!(history.current_route(), initial);
            assert_eq!(writes.borrow().len(), 1);
            let world = rendered_event_target(&dom, "select", "id", "public-world", "onchange");
            assert!(has_attribute(&edits, "value", "main", Some(world)));
            assert!(has_text(&edits, " / Main"));
            assert!(!has_text(&edits, " / Isekai"));
            assert!(!has_attribute(&edits, "aria-label", "Auction #1", None));
            assert_eq!(requests.borrow().len(), 1, "只读取用来定位目标场次的未筛选目录");
            assert!(!requests.borrow()[0].0.contains("category="));
            assert!(has_text(&edits, "Could not update the world in this link."));

            let retry =
                rendered_event_target(&dom, "button", "class", "ui-async-state__retry", "onclick");
            allowed.set(true);
            dispatch_page_event(&dom, "click", retry, "");
            let edits = settle_page(&mut dom);
            assert_eq!(history.current_route(), expected);
            assert_eq!(*writes.borrow(), [expected.clone(), expected.clone()]);
            assert!(has_attribute(&edits, "value", "isekai", Some(world)));
            assert!(has_text(&edits, " / Isekai"));
            assert!(has_text(&edits, "Auction #1"));
            assert!(has_attribute(&edits, "aria-label", "Auction #1", None));
            assert_eq!(requests.borrow().len(), 2);
            assert!(requests.borrow()[1].0.contains("category=clo"));
            assert_eq!(requests.borrow()[1].1, expected);
        },
    );
}

#[test]
fn the_world_picker_replaces_only_the_disabled_world() {
    const RAW: &str = "%74ype=%63lo&&keep=%2b%20x&keep=";
    for (from, to) in [("main", "isekai"), ("isekai", "main")] {
        for accepted in [false, true] {
            let initial = format!("/?%77orld={from}&{RAW}#lots");
            let corrected_world = if to == "main" { "" } else { "&world=isekai" };
            let expected = format!("/?{RAW}{corrected_world}#lots");
            let history = Rc::new(MemoryHistory::with_initial_path(&initial));
            let mut dom = shell_fixture(to).with_root_context(PageReadFixture::new(|_: String| {
                Ok::<_, String>(None::<CurrentAuction>)
            }));
            let writes = Rc::new(RefCell::new(Vec::new()));
            provide_test_public_history::<Route>(&mut dom, history.clone(), {
                let writes = writes.clone();
                move |url, mode| {
                    assert_eq!(mode, QueryWriteMode::Replace);
                    writes.borrow_mut().push(url.to_owned());
                    accepted
                }
            });
            dom.rebuild_in_place();
            settle(&mut dom);
            assert_eq!(writes.borrow().as_slice(), std::slice::from_ref(&expected));
            assert_eq!(history.current_route(), if accepted { expected } else { initial });
            assert!(!history.can_go_back());
        }
    }
}

#[test]
fn catalog_total_clamp_replaces_only_page_and_reads_the_corrected_page() {
    const INITIAL: &str = "/?%70age=8&%74ype=%63lo&&keep=%2b%20x&keep=#lots";
    const CORRECTED: &str = "/?%74ype=%63lo&&keep=%2b%20x&keep=#lots";
    for accepted in [false, true] {
        let history = Rc::new(MemoryHistory::with_initial_path(INITIAL));
        let requests = Rc::new(RefCell::new(Vec::new()));
        let lot_reads = PageReadFixture::new({
            let requests = requests.clone();
            let history = history.clone();
            move |url: String| {
                let page = if url.contains("page=8&") { 8 } else { 1 };
                requests.borrow_mut().push((url, history.current_route()));
                let mut envelope = catalog_envelope(AUCTION, "live", 0, page, 20);
                envelope.total = Some(3);
                Ok::<_, String>(envelope)
            }
        });
        let mut dom = shell_fixture("main")
            .with_root_context(PageReadFixture::new(|_: String| {
                Ok::<_, String>(Some(active_auction("live")))
            }))
            .with_root_context(lot_reads);
        let writes = Rc::new(RefCell::new(Vec::new()));
        provide_test_public_history::<Route>(&mut dom, history.clone(), {
            let writes = writes.clone();
            move |url, mode| {
                assert_eq!(mode, QueryWriteMode::Replace);
                writes.borrow_mut().push(url.to_owned());
                accepted
            }
        });
        dom.rebuild_in_place();
        settle(&mut dom);
        assert_eq!(*writes.borrow(), [CORRECTED]);
        assert_eq!(history.current_route(), if accepted { CORRECTED } else { INITIAL });
        let mut expected = vec![(8, INITIAL)];
        if accepted {
            expected.push((1, CORRECTED));
        }
        let requests = requests.borrow();
        assert_eq!(requests.len(), expected.len());
        for ((url, address), (page, expected_address)) in requests.iter().zip(expected) {
            assert_eq!(
                url,
                &format!(
                    "/api/v1/auctions/{AUCTION}/lots?page={page}&page_size=20&category=clo&fields=summary"
                )
            );
            assert_eq!(address, expected_address);
        }
    }
}

fn has_attribute(
    edits: &[Mutation],
    key: &str,
    text: &str,
    element: Option<dioxus::dioxus_core::ElementId>,
) -> bool {
    edits.iter().any(|edit| {
        matches!(edit,
            Mutation::SetAttribute { name, value: AttributeValue::Text(value), id, .. }
                if *name == key && value == text && element.is_none_or(|element| *id == element)
        )
    })
}

fn has_text(edits: &[Mutation], text: &str) -> bool {
    edits.iter().any(|edit| matches!(edit,
        Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. } if value == text
    ))
}

fn player(user_id: &str) -> PublicSession {
    PublicSession {
        authenticated: true,
        user_id: user_id.to_owned(),
        username: user_id.to_owned(),
        expires_at: None,
        absolute_expires_at: None,
    }
}

fn confirm_session(dom: &mut VirtualDom, user_id: Option<&str>) {
    let Controls { mut session, owner, .. } = controls();
    dom.in_runtime(|| session.set(Some(user_id.map(player))));
    settle(dom);
    dom.in_runtime(|| assert_eq!(owner.read().as_deref(), user_id));
}

fn finish_request(dom: &mut VirtualDom, index: usize) {
    SCRIPT.with_borrow(|script| script.as_ref().unwrap().requests[index].gate.open());
    settle(dom);
}

fn notify_bid(dom: &mut VirtualDom) {
    dom.in_scope(ScopeId::APP, || {
        let controls = controls();
        controls.bids.notify(AUCTION, *controls.refresh.peek());
    });
    settle(dom);
}

fn finish_timer(dom: &mut VirtualDom, index: usize) {
    SCRIPT.with_borrow(|script| script.as_ref().unwrap().timers[index].open());
    settle(dom);
}

#[test]
fn late_anonymous_session_keeps_the_single_inflight_request() {
    let mut dom = mounted(None);
    confirm_session(&mut dom, None);
    finish_request(&mut dom, 0);
    assert_eq!(started(), 1);
}

#[test]
fn late_authenticated_cold_start_keeps_the_single_inflight_request() {
    let mut dom = mounted(None);
    confirm_session(&mut dom, Some(ALICE));
    finish_request(&mut dom, 0);
    assert_eq!(started(), 1, "冷启动确认已登录身份不得重读");
}

#[test]
fn login_confirmation_and_logout_keep_the_single_inflight_request() {
    let mut dom = mounted(None);
    confirm_session(&mut dom, None);
    confirm_session(&mut dom, Some(ALICE));
    assert_eq!(started(), 1, "确认登录不得取消同址请求");
    confirm_session(&mut dom, None);
    finish_request(&mut dom, 0);
    assert_eq!(started(), 1, "退出不得重读同址目录");
}

#[test]
fn account_switch_keeps_the_pending_bid_timer_and_consumes_the_revision_once() {
    let mut dom = mounted(Some(ALICE));
    finish_request(&mut dom, 0);
    notify_bid(&mut dom);
    assert_eq!(started(), 1, "定时器到期前不读");

    confirm_session(&mut dom, Some(BOB));
    assert_eq!(started(), 1, "换号不提前重读");
    finish_timer(&mut dom, 0);
    assert_eq!(started(), 2, "已排定的补读仍须执行");
    finish_request(&mut dom, 1);
    assert_eq!(SCRIPT.with_borrow(|script| script.as_ref().unwrap().timers.len()), 1);

    notify_bid(&mut dom);
    finish_timer(&mut dom, 1);
    finish_request(&mut dom, 2);
    assert_eq!(started(), 3, "换号后的单条通知恰好补读一次");
}

#[test]
fn a_pending_bid_in_the_previous_auction_cannot_restart_the_new_catalog() {
    let mut dom = mounted(Some(ALICE));
    finish_request(&mut dom, 0);
    notify_bid(&mut dom);
    dom.in_runtime(|| {
        let mut current = controls().auction;
        current.write().1.as_mut().unwrap().as_mut().unwrap().id = BOB.to_owned();
    });
    settle(&mut dom);
    finish_request(&mut dom, 1);
    finish_timer(&mut dom, 0);
    assert_eq!(started(), 2);
    SCRIPT.with_borrow(|script| {
        assert_eq!(script.as_ref().unwrap().requests[1].auction_id, BOB);
    });
}

struct BoardLocation {
    location: RefCell<String>,
    writable: Cell<bool>,
}

impl QueryLocation for BoardLocation {
    fn current(&self) -> String {
        self.location.borrow().clone()
    }

    fn write(&self, location: &str, _mode: QueryWriteMode) -> bool {
        if self.writable.get() {
            self.location.replace(location.to_owned());
        }
        self.writable.get()
    }
}

fn mount_at(search: &str, writable: bool) -> (VirtualDom, Rc<BoardLocation>, Vec<Mutation>) {
    SCRIPT.with_borrow_mut(|script| {
        *script = Some(Script { initial_user: Some(ALICE), ..Script::default() });
    });
    let address = Rc::new(BoardLocation {
        location: RefCell::new(format!("/{search}")),
        writable: Cell::new(writable),
    });
    let mut dom = VirtualDom::new(TestApp);
    dom.provide_root_context(address.clone() as Rc<dyn QueryLocation>);
    let mut edits = dom.rebuild_to_vec().edits;
    edits.extend(crate::surface::settle_page(&mut dom));
    (dom, address, edits)
}

#[test]
fn malformed_category_does_not_issue_a_wider_catalog_request() {
    crate::surface::run_page_events(
        module_path!(),
        "malformed_category_does_not_issue_a_wider_catalog_request",
        || {
            for (search, message) in [
                (
                    "?type=unknown",
                    "The equipment type filter is invalid. Clear it to load the catalog.",
                ),
                ("?type=clo&type=shd", "This filter appears more than once. Enter a single value."),
            ] {
                let (mut dom, address, edits) = mount_at(search, false);
                assert_eq!(started(), 0);
                assert_eq!(address.current(), format!("/{search}"));
                assert!(edits.iter().any(|edit| matches!(edit,
                    Mutation::CreateTextNode { value, .. } if value == message
                )));
                let button = crate::surface::rendered_event_target(
                    &dom,
                    "button",
                    "id",
                    "public-catalog-clear-category",
                    "onclick",
                );
                crate::surface::dispatch_page_event(&dom, "click", button, "");
                settle(&mut dom);
                assert_eq!(started(), 0);
                assert_eq!(address.current(), format!("/{search}"));
                address.writable.set(true);
                crate::surface::dispatch_page_event(&dom, "click", button, "");
                settle(&mut dom);
                assert_eq!(address.current(), "/");
                assert_eq!(started(), 1);
            }
        },
    );
}

#[test]
fn catalog_does_not_read_a_deep_link_until_its_page_is_within_the_offset_limit() {
    let (_dom, address, _) = mount_at("?page=502&page_size=20", false);
    assert_eq!(started(), 0);
    assert_eq!(address.current(), "/?page=502&page_size=20");
    let (_dom, address, _) = mount_at("?page=502&page_size=20", true);
    assert_eq!(address.current(), "/?page_size=20&page=501");
    assert_eq!(started(), 1);
    SCRIPT.with_borrow(|script| {
        assert_eq!(script.as_ref().unwrap().requests[0].page, 501);
    });
    let (_dom, address, _) = mount_at("?type=clo&page=51&page_size=200", false);
    assert_eq!(address.current(), "/?type=clo&page=51&page_size=200");
    assert_eq!(started(), 1);
    SCRIPT.with_borrow(|script| {
        let script = script.as_ref().unwrap();
        assert_eq!(
            (script.requests[0].category.as_deref(), script.requests[0].page),
            (Some("clo"), 51)
        );
    });
}

#[test]
fn hidden_round_switch_does_not_read_settled_rounds_or_report_their_error() {
    for (mode, expected_switch_reads, fail) in [
        (None, 0, true),
        (Some("live"), 0, true),
        (Some("preview"), 1, true),
        (Some("preview"), 1, false),
    ] {
        SCRIPT.set(Some(Script {
            initial_user: Some(ALICE),
            catalog_status: Some("settled"),
            ..Script::default()
        }));
        let address = Rc::new(BoardLocation {
            location: RefCell::new(format!("/?world=main&auction={BOB}")),
            writable: Cell::new(true),
        });
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut dom = VirtualDom::new(TestApp);
        dom.provide_root_context(address as Rc<dyn QueryLocation>);
        dom.provide_root_context(PageReadFixture::new(|url: String| {
            assert!(url.contains(&format!("/{BOB}/lots?")));
            Ok::<_, String>(catalog_envelope(BOB, "settled", 0, 1, 20))
        }));
        dom.provide_root_context(PageReadFixture::new(move |url: String| {
            assert!(url.contains("world=main"));
            Ok::<_, String>(mode.map(active_auction))
        }));
        dom.provide_root_context(PageReadFixture::new({
            let calls = calls.clone();
            move |url: String| {
                let scoped = url.contains("world=main");
                calls.borrow_mut().push(url);
                if scoped && fail {
                    Err("fixture".to_owned())
                } else {
                    Ok(historical_page(BOB, "main"))
                }
            }
        }));
        let mut edits = dom.rebuild_to_vec().edits;
        edits.extend(crate::surface::settle_page(&mut dom));
        assert_eq!(
            calls.borrow().iter().filter(|url| url.contains("world=main")).count(),
            expected_switch_reads
        );
        assert_eq!(edits.iter().any(|edit| matches!(
            edit, Mutation::CreateTextNode { value, .. } if value == "Auction data is temporarily unavailable."
        )), expected_switch_reads == 1 && fail);
        if mode == Some("preview") && !fail {
            assert!(edits.iter().any(|edit| matches!(edit,
                Mutation::SetAttribute { name: "href", value: AttributeValue::Text(value), .. }
                    if value.starts_with("/auction?world=main") && !value.contains("auction=")
            )));
        }
    }
}

#[test]
fn exhausts_alignment_next_to_the_retained_preview_then_accepts_the_replacement() {
    crate::surface::run_page_events(
        module_path!(),
        "exhausts_alignment_next_to_the_retained_preview_then_accepts_the_replacement",
        exercise_preview_alignment,
    );
}

fn exercise_preview_alignment() {
    SCRIPT.set(Some(Script {
        initial_user: Some(ALICE),
        initial_auction: Some(active_auction("preview")),
        catalog_status: Some("preview"),
        ..Script::default()
    }));
    let mut dom = VirtualDom::new(TestApp)
        .with_root_context(PageReadFixture::new(|_url: String| {
            Ok::<_, String>(Some(active_auction("preview")))
        }))
        .with_root_context(PageReadFixture::new(|_url: String| {
            Ok::<_, String>(historical_page(BOB, "main"))
        }));
    dom.rebuild_in_place();
    settle(&mut dom);
    SCRIPT.with_borrow(|script| script.as_ref().unwrap().requests[0].gate.open());
    let shown = crate::surface::settle_page(&mut dom);
    assert!(shown.iter().any(|edit| matches!(edit, Mutation::CreateTextNode { value, .. } if value == "Preview revision 0")));

    dom.in_runtime(|| {
        controls().auction.write().1.as_mut().unwrap().as_mut().unwrap().preview_revision = 1;
    });
    settle(&mut dom);
    assert_eq!(started(), 2);
    SCRIPT.with_borrow(|script| script.as_ref().unwrap().requests[1].gate.open());
    let edits = crate::surface::settle_page(&mut dom);
    assert!(edits.iter().any(|edit| matches!(edit, Mutation::CreateTextNode { value, .. } if value.contains("table still shows the previous revision"))));
    assert!(
        edits
            .iter()
            .any(|edit| matches!(edit, Mutation::CreateTextNode { value, .. } if value == "Retry"))
    );
    assert!(!edits.iter().any(|edit| matches!(edit, Mutation::Remove { .. })));

    SCRIPT.with_borrow_mut(|script| script.as_mut().unwrap().catalog_revision = 1);
    let retry = crate::surface::rendered_event_target(
        &dom,
        "button",
        "class",
        "ui-async-state__retry",
        "onclick",
    );
    crate::surface::dispatch_page_event(&dom, "click", retry, "");
    settle(&mut dom);
    assert_eq!(started(), 3);
    SCRIPT.with_borrow(|script| script.as_ref().unwrap().requests[2].gate.open());
    let recovered = crate::surface::settle_page(&mut dom);
    assert!(recovered.iter().any(|edit| matches!(edit,
        Mutation::SetText { value, .. } | Mutation::CreateTextNode { value, .. }
            if value == "Preview revision 1"
    )));
}

fn refresh_from_resume() {
    super::refresh_public_reads(controls().refresh);
}

#[test]
fn hidden_bids_wait_for_the_visible_authoritative_refresh() {
    SCRIPT.set(Some(Script { initial_user: Some(ALICE), ..Script::default() }));
    let visible = Rc::new(Cell::new(true));
    let visibility = visible.clone();
    let mut dom = VirtualDom::new(TestApp)
        .with_root_context(PageReadFixture::new(move |()| visibility.get()));
    dom.rebuild_in_place();
    settle(&mut dom);
    finish_request(&mut dom, 0);
    notify_bid(&mut dom);
    visible.set(false);
    finish_timer(&mut dom, 0);
    notify_bid(&mut dom);
    assert_eq!(started(), 1);
    assert_eq!(SCRIPT.with_borrow(|script| script.as_ref().unwrap().timers.len()), 1);
    visible.set(true);
    dom.in_runtime(refresh_from_resume);
    settle(&mut dom);
    assert_eq!(started(), 2);
}

#[derive(Default)]
struct ReplayTrace {
    body: Rc<Gate>,
    calls: RefCell<Vec<(i64, String, String)>>,
    target: Cell<Option<Signal<PublicRealtimeTarget>>>,
    state: Cell<Option<Signal<LiveConnectionState>>>,
    checks: RefCell<Vec<SessionRefreshReason>>,
}

fn subscription_harness() -> Element {
    let trace = consume_context::<Rc<ReplayTrace>>();
    use_context_provider(PageVisibility::new);
    use_context_provider(PublicBidRefresh::new);
    let check = use_callback({
        let trace = trace.clone();
        move |reason| trace.checks.borrow_mut().push(reason)
    });
    use_context_provider(|| PublicSessionCheckTrigger(check));
    let target = use_signal(|| PublicRealtimeTarget::from_auction(&active_auction("live")));
    let realtime_state = use_signal(|| LiveConnectionState::Connecting);
    let auction_after = use_signal(|| None);
    let refresh_generation = use_signal(|| 0);
    trace.target.set(Some(target));
    trace.state.set(Some(realtime_state));
    rsx! {
        PublicRealtimeOwner {
            target: target(), user_id: Some(ALICE.to_owned()), refresh_generation,
            auction_after, realtime_state,
        }
    }
}

#[test]
fn a_replaced_subscription_cannot_use_the_old_close_code_to_check_session() {
    for replaced in [false, true] {
        let trace = Rc::new(ReplayTrace::default());
        let mock = trace.clone();
        let mut dom = VirtualDom::new(subscription_harness)
            .with_root_context(trace.clone())
            .with_root_context(PageReadFixture::new(move |requested: (i64, String, String)| {
                mock.calls.borrow_mut().push(requested);
                let body = mock.body.clone();
                ReplayFixture {
                    status: 403,
                    close_code: 1008,
                    body: Box::pin(async move {
                        body.wait().await;
                        Err(TransportError::Timeout)
                    }),
                }
            }));
        dom.rebuild_in_place();
        settle(&mut dom);
        assert_eq!(*trace.calls.borrow(), [(0, AUCTION.to_owned(), "main".to_owned())]);
        if replaced {
            let mut target = trace.target.get().unwrap();
            dom.in_runtime(|| {
                target.set(PublicRealtimeTarget::Inactive {
                    world: "main".to_owned(),
                    preview: false,
                });
            });
            settle(&mut dom);
        }
        trace.body.open();
        settle(&mut dom);
        assert_eq!(trace.calls.borrow().len(), 1, "403 响应头确认后不自动重连");
        let expected: &[SessionRefreshReason] =
            if replaced { &[] } else { &[SessionRefreshReason::Refresh] };
        assert_eq!(trace.checks.borrow().as_slice(), expected);
        dom.in_runtime(|| {
            assert!(*trace.state.get().unwrap().peek() == LiveConnectionState::Manual);
        });
    }
}

#[test]
fn page_filter_and_refresh_generation_each_start_a_request() {
    let mut dom = mounted(None);
    let mut query = SCRIPT.with_borrow(|script| script.as_ref().unwrap().query.unwrap());
    dom.in_runtime(|| query.write().page = 2);
    settle(&mut dom);
    assert_eq!(started(), 2);
    dom.in_runtime(|| query.write().category = Some("shd".to_owned()).into());
    settle(&mut dom);
    assert_eq!(started(), 3, "页码不变时只改分类也必须重读");
    dom.in_runtime(|| controls().refresh.set(1));
    settle(&mut dom);
    assert_eq!(started(), 4);
    SCRIPT.with_borrow(|script| {
        let requests = &script.as_ref().unwrap().requests;
        let args: Vec<_> =
            requests.iter().map(|request| (request.category.as_deref(), request.page)).collect();
        assert_eq!(args, vec![(None, 1), (None, 2), (Some("shd"), 2), (Some("shd"), 2)]);
    });
}
