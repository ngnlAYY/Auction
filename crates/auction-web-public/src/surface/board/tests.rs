//! 驱动真实看板与资源，使用受控替身记录浏览器请求和竞价定时器。

use std::cell::{Cell, RefCell};
use std::future::{Future, pending, poll_fn};
use std::rc::Rc;
use std::task::{Poll, Waker};

use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
use auction_web_shared::query::QueryLocation;
use auction_web_shared::{PageVisibility, QueryWriteMode, use_second_ticker};
use dioxus::dioxus_core::{AttributeValue, Mutation, Mutations, NoOpMutations};
use dioxus::history::MemoryHistory;
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

use super::refresh::PublicBidRefresh;
use super::{AuctionIndex, AuctionPage, PublicDisplayedAuction, PublicPageRealtimeTarget};
use crate::api::{CurrentAuction, LotEnvelope, PublicSession};
use crate::surface::router::PUBLIC_LOCALE_CATALOG;
use crate::surface::watches::SkipWatchReads;
use crate::surface::{
    CurrentAuctionResource, LotViewRequest, PublicBoardQuery, PublicCommandOwner,
    PublicContentLoading, PublicReadinessResource, PublicSessionResource,
};

const AUCTION: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000001";
const ALICE: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000002";
const BOB: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000003";
const OPENS_AT: &str = "2026-11-13T00:00:00Z";

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
    category: Option<String>,
    page: i64,
}

#[derive(Clone, Copy)]
struct Controls {
    session: Signal<Option<Option<PublicSession>>>,
    owner: Signal<Option<String>>,
    refresh: Signal<u64>,
    bids: PublicBidRefresh,
}

#[derive(Default)]
struct Script {
    initial_user: Option<&'static str>,
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
    let gate = SCRIPT.with_borrow_mut(|script| {
        let script = script.as_mut()?;
        let gate = Rc::new(Gate::default());
        script.requests.push(Request {
            gate: Rc::clone(&gate),
            category: category.map(str::to_owned),
            page,
        });
        Some(gate)
    })?;
    let auction_id = auction_id.to_owned();
    Some(async move {
        gate.wait().await;
        Ok(LotEnvelope {
            auction_id,
            auction_status: "live".to_owned(),
            projection_kind: "live".to_owned(),
            presentation_mode: "live".to_owned(),
            bidding_open: true,
            opens_at: OPENS_AT.to_owned(),
            preview_revision: 0,
            final_restriction: None,
            lots: None,
            lot: None,
            total: Some(200),
            page: Some(page),
            page_size: Some(page_size),
            category_counts: None,
        })
    })
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
    let auction: CurrentAuctionResource = use_resource(|| async {
        let auction = CurrentAuction {
            id: AUCTION.to_owned(),
            world: "main".to_owned(),
            number: 1,
            status: "live".to_owned(),
            opens_at: OPENS_AT.to_owned(),
            scheduled_ends_at: None,
            presentation_mode: "live".to_owned(),
            bidding_open: true,
            preview_revision: 0,
            proxy_default_mode: Some("fast".to_owned()),
        };
        ("main".to_owned(), Ok(Some(auction)))
    });
    use_context_provider(|| auction);
    use_context_provider(|| Signal::new("main".to_owned()));
    use_context_provider(|| LotViewRequest(Signal::new(None)));
    use_context_provider(|| PublicDisplayedAuction(Signal::new(None)));
    use_context_provider(|| PublicPageRealtimeTarget(Signal::new(None)));
    use_context_provider(|| PublicContentLoading(Signal::new(Default::default())));
    let refresh = use_context_provider(|| Signal::new(0_u64));
    let bids = use_context_provider(|| PublicBidRefresh(Signal::new(0_u64)));
    SCRIPT.with_borrow_mut(|script| {
        script.as_mut().unwrap().controls =
            Some(Controls { session: session_input, owner, refresh, bids });
    });
    rsx! { Router::<TestRoute> {} }
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
    dom.in_runtime(|| controls().bids.notify());
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
fn page_filter_and_refresh_generation_each_start_a_request() {
    let mut dom = mounted(None);
    let mut query = SCRIPT.with_borrow(|script| script.as_ref().unwrap().query.unwrap());
    dom.in_runtime(|| query.write().page = 2);
    settle(&mut dom);
    assert_eq!(started(), 2);
    dom.in_runtime(|| query.write().category = Some("shield".to_owned()));
    settle(&mut dom);
    assert_eq!(started(), 3, "页码不变时只改分类也必须重读");
    dom.in_runtime(|| controls().refresh.set(1));
    settle(&mut dom);
    assert_eq!(started(), 4);
    SCRIPT.with_borrow(|script| {
        let requests = &script.as_ref().unwrap().requests;
        let args: Vec<_> =
            requests.iter().map(|request| (request.category.as_deref(), request.page)).collect();
        assert_eq!(args, vec![(None, 1), (None, 2), (Some("shield"), 2), (Some("shield"), 2)]);
    });
}
