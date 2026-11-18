//! 竞价日志与履约选择器的分批目录、场次确认及地址纠偏。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use auction_web_shared::t;
use dioxus::dioxus_core::Task;
use dioxus::prelude::*;

use crate::api::{HistoricalAuction, fetch_bid_log_auctions, fetch_fulfillment_auctions};
use crate::surface::{
    AuctionParameter, AuctionSelection, PublicBoardQuery, QueryCommit, commit_auction_selection,
    page_pathname, read_page_query,
};

const BID_LOG_STATUSES: &[&str] = &["live", "final", "settled"];
const FULFILLMENT_STATUSES: &[&str] = &["settled"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RoundPage {
    BidLog,
    Fulfillment,
}

impl RoundPage {
    pub(super) fn pathname(self) -> &'static str {
        match self {
            Self::BidLog => "/bidlog",
            Self::Fulfillment => "/fulfillment",
        }
    }

    fn statuses(self) -> &'static [&'static str] {
        match self {
            Self::BidLog => BID_LOG_STATUSES,
            Self::Fulfillment => FULFILLMENT_STATUSES,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct DirectoryScope {
    world: Option<String>,
    generation: u64,
}

struct StatusPages {
    status: &'static str,
    rows: Vec<HistoricalAuction>,
    next_page: Option<i64>,
    running: bool,
    failed: bool,
}

struct Directory {
    scope: DirectoryScope,
    statuses: Vec<StatusPages>,
    options: Rc<[HistoricalAuction]>,
    previous_options: Rc<[HistoricalAuction]>,
}

enum Resolution<'a> {
    Waiting,
    Failed,
    Confirmed(Option<&'a HistoricalAuction>),
}

impl Directory {
    fn new(
        scope: DirectoryScope,
        page: RoundPage,
        previous_options: Rc<[HistoricalAuction]>,
    ) -> Self {
        Self {
            scope,
            statuses: page
                .statuses()
                .iter()
                .map(|status| StatusPages {
                    status,
                    rows: Vec::new(),
                    next_page: Some(1),
                    running: false,
                    failed: false,
                })
                .collect(),
            options: Rc::from([]),
            previous_options,
        }
    }

    fn resolve(&self, requested_id: Option<&str>) -> Resolution<'_> {
        if let Some(id) = requested_id
            && let Some(found) = self.options.iter().find(|auction| auction.id == id)
        {
            return Resolution::Confirmed(Some(found));
        }
        // 同一世界内 API 按期号和 ID 降序；默认场次只需比较各状态首页。
        // 指定场次未找到时则必须查实目录结束，不能把未读取的场次判成不可见。
        let incomplete = |status: &&StatusPages| {
            if requested_id.is_none() {
                status.next_page == Some(1)
            } else {
                status.next_page.is_some()
            }
        };
        if self.statuses.iter().filter(incomplete).any(|status| status.failed) {
            return Resolution::Failed;
        }
        if self.statuses.iter().any(|status| incomplete(&status)) {
            return Resolution::Waiting;
        }
        Resolution::Confirmed(self.options.first())
    }

    fn requests(&self, requested_id: Option<&str>, load_options: bool) -> Vec<(&'static str, i64)> {
        let seek =
            requested_id.is_some() && matches!(self.resolve(requested_id), Resolution::Waiting);
        self.statuses
            .iter()
            .filter_map(|status| {
                let page = status.next_page?;
                (!status.running && !status.failed && (page == 1 || seek || load_options))
                    .then_some((status.status, page))
            })
            .collect()
    }

    fn accept(
        &mut self,
        status: &'static str,
        page: i64,
        result: Result<(Vec<HistoricalAuction>, bool), String>,
    ) {
        let Some(state) = self.statuses.iter_mut().find(|pages| pages.status == status) else {
            return;
        };
        if state.next_page != Some(page) {
            return;
        }
        state.running = false;
        match result {
            Ok((page_rows, complete)) => {
                state.rows.extend(page_rows);
                state.next_page = if complete { None } else { page.checked_add(1) };
                self.options = order_bid_log_auctions(
                    self.statuses.iter().flat_map(|status| status.rows.iter().cloned()).collect(),
                )
                .into();
            }
            Err(_error) => state.failed = true,
        }
    }

    fn needs_more_options(&self) -> bool {
        self.statuses.iter().any(|status| status.next_page.is_some_and(|page| page > 1))
    }

    fn display_options(&self) -> Rc<[HistoricalAuction]> {
        if self.options.is_empty()
            && self.statuses.iter().any(|status| status.next_page == Some(1) && !status.failed)
        {
            self.previous_options.clone()
        } else {
            self.options.clone()
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ConfirmedRound {
    pub(super) id: String,
    pub(super) world: String,
    generation: u64,
}

#[derive(Clone, PartialEq)]
struct DirectoryView {
    options: Rc<[HistoricalAuction]>,
    selected: Option<HistoricalAuction>,
    readable: Option<ConfirmedRound>,
    correction: Option<RoundCorrection>,
    empty: bool,
    failed: bool,
    options_failed: bool,
}

#[derive(Clone, PartialEq)]
struct RoundCorrection {
    scope: DirectoryScope,
    expected: AuctionParameter,
    next: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) struct RoundDirectory {
    view: Memo<Option<DirectoryView>>,
    confirmed: Memo<Option<ConfirmedRound>>,
    requested: Memo<AuctionParameter>,
    rejected: Signal<bool>,
    opened_world: Signal<Option<String>>,
    scope: Memo<DirectoryScope>,
}

impl RoundDirectory {
    pub(super) fn options(self) -> Rc<[HistoricalAuction]> {
        self.view.read().as_ref().map_or_else(|| Rc::from([]), |value| value.options.clone())
    }

    pub(super) fn selectable(self, id: &str) -> bool {
        self.view
            .read()
            .as_ref()
            .is_some_and(|value| value.options.iter().any(|auction| auction.id == id))
    }

    pub(super) fn selected(self) -> Option<HistoricalAuction> {
        self.view.read().as_ref().and_then(|value| value.selected.clone())
    }

    pub(super) fn readable(self) -> Option<ConfirmedRound> {
        (self.confirmed)()
    }

    pub(super) fn invalid_auction(self) -> bool {
        matches!(&*self.requested.read(), AuctionParameter::Invalid(_))
    }

    pub(super) fn empty(self) -> bool {
        self.view.read().as_ref().is_some_and(|value| value.empty)
    }

    pub(super) fn failed(self) -> bool {
        self.view.read().as_ref().is_some_and(|value| value.failed)
    }

    pub(super) fn options_failed(self) -> bool {
        self.view.read().as_ref().is_some_and(|value| value.options_failed)
    }

    pub(super) fn correction_failed(self) -> bool {
        (self.rejected)()
            && self.view.read().as_ref().is_some_and(|value| value.correction.is_some())
    }

    pub(super) fn load_options(mut self) {
        let world = self.scope.peek().world.clone();
        self.opened_world.set(world);
    }
}

/// 目录逐页确认场次。选定后暂停翻页，打开选择器时再补齐剩余选项。
pub(super) fn use_round_directory(
    page: RoundPage,
    query: Signal<PublicBoardQuery>,
    world: Memo<Option<String>>,
    refresh: Signal<u64>,
) -> RoundDirectory {
    use_round_directory_with(page, query, world, refresh, fetch_bid_log_auctions)
}

fn use_round_directory_with(
    page: RoundPage,
    query: Signal<PublicBoardQuery>,
    world: Memo<Option<String>>,
    refresh: Signal<u64>,
    fetch_page: impl AsyncFn(
        String,
        &'static str,
        i64,
    ) -> Result<(Vec<HistoricalAuction>, bool), String>
    + Clone
    + 'static,
) -> RoundDirectory {
    let scope = use_memo(move || DirectoryScope { world: world(), generation: refresh() });
    let requested = use_memo(move || query.read().auction_parameter().clone());
    let mut directory = use_signal(|| Directory::new(scope.peek().clone(), page, Rc::from([])));
    let mut opened_world = use_signal(|| None::<String>);
    let tasks = use_hook(|| Rc::new(RefCell::new(BTreeMap::<&str, Task>::new())));
    let full_fulfillment_request = use_memo(move || {
        let scope = scope();
        let current = directory.read();
        (page == RoundPage::Fulfillment
            && scope.world.is_some()
            && current.scope == scope
            && current.needs_more_options()
            && *opened_world.read() == scope.world)
            .then_some(scope)
    });
    let full_fulfillment = use_resource(move || {
        let scope = full_fulfillment_request();
        async move {
            let scope = scope?;
            if read_page_query().world.invalid() {
                return None;
            }
            let result =
                fetch_fulfillment_auctions(scope.world.clone()?).await.map(Rc::<[_]>::from);
            Some((scope, result))
        }
    });
    use_effect(move || {
        let current = scope();
        let wanted = requested();
        let opened = *opened_world.read() == current.world;
        let scope_changed = directory.peek().scope != current;
        if scope_changed {
            for (_, task) in std::mem::take(&mut *tasks.borrow_mut()) {
                task.cancel();
            }
            let previous = {
                let old = directory.peek();
                if old.scope.world == current.world {
                    old.display_options()
                } else {
                    if opened_world.peek().is_some() {
                        opened_world.set(None);
                    }
                    Rc::from([])
                }
            };
            directory.set(Directory::new(current.clone(), page, previous));
        }
        // 非法世界仍保留在地址中，取消上一范围的目录且不发生任何场次查询。
        let Some(world) = current.world.as_ref() else { return };
        if read_page_query().world.invalid() {
            return;
        }
        if full_fulfillment
            .read_unchecked()
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|(scope, result)| *scope == current && result.is_ok())
        {
            return;
        }
        let requests =
            directory.read().requests(wanted.selected_id(), opened && page == RoundPage::BidLog);
        for (status, number) in requests {
            if let Some(progress) =
                directory.write().statuses.iter_mut().find(|entry| entry.status == status)
            {
                progress.running = true;
            }
            let expected = current.clone();
            let fetch_page = fetch_page.clone();
            let world = world.clone();
            let task = spawn(async move {
                let result = fetch_page(world, status, number).await;
                // 迟到的目录只属于发起时的世界与代际。
                let same_scope = *scope.peek() == expected && directory.peek().scope == expected;
                if same_scope {
                    directory.write().accept(status, number, result);
                }
            });
            tasks.borrow_mut().insert(status, task);
        }
    });
    let view = use_memo(move || {
        let scope = scope();
        scope.world.as_ref()?;
        let target = requested();
        let id = target.selected_id();
        let invalid = matches!(&target, AuctionParameter::Invalid(_));
        let current = directory.read();
        if current.scope.world != scope.world {
            return None;
        }
        let full = full_fulfillment.read_unchecked();
        let full = full.as_ref().and_then(Option::as_ref).filter(|(loaded, _)| *loaded == scope);
        let (resolution, option_rows, options_failed) = if current.scope != scope {
            (Resolution::Waiting, current.display_options(), false)
        } else {
            // 选择器按需重用原有的单状态目录读取；已确认的正文不受补页失败影响。
            if let Some((_, Ok(full))) = full {
                (
                    Resolution::Confirmed(
                        full.iter()
                            .find(|auction| Some(auction.id.as_str()) == id)
                            .or_else(|| full.first()),
                    ),
                    full.clone(),
                    false,
                )
            } else {
                let failed = full.is_some_and(|(_, result)| result.is_err())
                    || current.statuses.iter().any(|page| page.failed);
                let resolution = current.resolve(id);
                let rows = if matches!(resolution, Resolution::Waiting)
                    && current
                        .previous_options
                        .iter()
                        .any(|auction| Some(auction.id.as_str()) == id)
                    && !current.options.iter().any(|auction| Some(auction.id.as_str()) == id)
                {
                    current.previous_options.clone()
                } else {
                    current.display_options()
                };
                (resolution, rows, failed)
            }
        };
        let selected = option_rows.iter().find(|auction| Some(auction.id.as_str()) == id).cloned();
        let (readable, correction, empty, failed) = match resolution {
            Resolution::Confirmed(choice) => {
                let next = choice.map(|auction| auction.id.clone());
                let mut readable = None;
                let mut correction = None;
                if invalid || next.as_deref() != id {
                    correction =
                        Some(RoundCorrection { scope: scope.clone(), expected: target, next });
                } else if let Some(auction) = choice {
                    readable = Some(ConfirmedRound {
                        id: auction.id.clone(),
                        world: auction.world_id.clone(),
                        generation: scope.generation,
                    });
                }
                (readable, correction, choice.is_none(), false)
            }
            Resolution::Waiting => (None, None, false, false),
            Resolution::Failed => (None, None, false, true),
        };
        Some(DirectoryView {
            options: option_rows,
            selected,
            readable,
            correction,
            empty,
            failed,
            options_failed,
        })
    });
    let correction =
        use_memo(move || view.read().as_ref().and_then(|value| value.correction.clone()));
    let mut rejected = use_signal(|| false);
    use_effect(move || {
        rejected.set(false);
        if let Some(change) = correction() {
            if change.scope != *scope.peek() {
                return;
            }
            // 非法地址由页面错误与恢复控件处理，只有拒写才是纠偏失败。
            let result = replace_round(query, page, &change);
            rejected
                .set(matches!(result, Some(QueryCommit::Rejected | QueryCommit::InvalidSelection)));
        }
    });
    let confirmed = use_memo(move || view.read().as_ref().and_then(|value| value.readable.clone()));
    RoundDirectory { view, confirmed, requested, rejected, opened_world, scope }
}

fn replace_round(
    query: Signal<PublicBoardQuery>,
    page: RoundPage,
    change: &RoundCorrection,
) -> Option<QueryCommit> {
    if page_pathname() != page.pathname() {
        return None;
    }
    let current = read_page_query();
    if current.world != query.peek().world || current.auction_parameter() != &change.expected {
        return None;
    }
    let keys: &[&str] = if page == RoundPage::BidLog {
        &["auction", "page", "bidlog_page"]
    } else {
        &["auction", "page"]
    };
    // 必须先写入已确认的场次才允许 GET，纠偏也不能把非法 lot 当成未提供。
    Some(commit_auction_selection(
        query,
        AuctionSelection::Automatic(change.next.clone()),
        Some(keys),
        |next| {
            next.page = 1;
            next.bid_log_page_alias = false;
        },
    ))
}

/// 补页失败时保留已确认的正文，并提供原来的刷新入口。
#[component]
pub(super) fn RoundOptionsError(on_retry: EventHandler<()>) -> Element {
    rsx! {
        div { class: "ui-pager__error", role: "status",
            span { {t!("data-error")} }
            button { r#type: "button", onclick: move |_| on_retry.call(()), {t!("data-retry")} }
        }
    }
}
fn order_bid_log_auctions(mut auctions: Vec<HistoricalAuction>) -> Vec<HistoricalAuction> {
    // 各状态独立读取，场次在查询之间迁移时只采用已确认的最高版本。
    auctions.sort_by(|left, right| {
        left.id.cmp(&right.id).then_with(|| right.state_version.cmp(&left.state_version))
    });
    auctions.dedup_by(|left, right| left.id == right.id);
    auctions.sort_by(|left, right| {
        bid_log_world_rank(&left.world_id)
            .cmp(&bid_log_world_rank(&right.world_id))
            .then_with(|| right.sequence.cmp(&left.sequence))
            .then_with(|| right.id.cmp(&left.id))
    });
    auctions
}

fn bid_log_world_rank(world_id: &str) -> u8 {
    match world_id {
        "main" => 0,
        "isekai" => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{pending, poll_fn};
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    use dioxus::prelude::*;

    use super::{RoundDirectory, RoundPage, order_bid_log_auctions, use_round_directory_with};
    use crate::api::HistoricalAuction;
    use crate::surface::reads::query::{ReadSelection, use_round_body};
    use crate::surface::{PageBrowserFixture, use_page_query};

    const NEWEST: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
    const OLDER: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";

    fn auction(
        id: &str,
        world_id: &str,
        sequence: i64,
        status: &str,
        version: i64,
    ) -> HistoricalAuction {
        HistoricalAuction {
            id: id.to_owned(),
            world_id: world_id.to_owned(),
            sequence,
            status: status.to_owned(),
            scheduled_open_at: "2026-11-14T12:00:00Z".to_owned(),
            opened_at: None,
            final_phase_at: None,
            effective_end_at: None,
            settled_at: None,
            lot_count: 0,
            sold_count: 0,
            unsold_count: 0,
            sold_amount: 0,
            state_version: version,
        }
    }

    #[test]
    fn bid_log_directory_keeps_the_highest_version_in_either_input_order() {
        let live = auction("same", "main", 8, "live", 2);
        let final_phase = auction("same", "main", 8, "final", 3);
        for input in [vec![live.clone(), final_phase.clone()], vec![final_phase.clone(), live]] {
            assert_eq!(
                order_bid_log_auctions(input).as_slice(),
                std::slice::from_ref(&final_phase)
            );
        }
    }

    #[test]
    fn bid_log_directory_orders_worlds_then_descending_sequences() {
        let main_older = auction("main-older", "main", 7, "settled", 9);
        let main_newer = auction("main-newer", "main", 8, "live", 2);
        let isekai = auction("isekai", "isekai", 9, "live", 2);
        assert_eq!(
            order_bid_log_auctions(vec![isekai.clone(), main_older.clone(), main_newer.clone()]),
            [main_newer, main_older, isekai],
        );
    }

    struct ReadTrace {
        page: RoundPage,
        accepts_replace: bool,
        browser: PageBrowserFixture,
        directory: Cell<Option<RoundDirectory>>,
        requests: RefCell<Vec<String>>,
        second_page: Cell<Option<Waker>>,
        release_page: Cell<bool>,
    }

    impl ReadTrace {
        fn mount(
            page: RoundPage,
            selected: Option<&'static str>,
            accepts_replace: bool,
        ) -> (VirtualDom, Rc<Self>) {
            let search = selected.map(|id| format!("auction={id}")).unwrap_or_default();
            let browser = PageBrowserFixture::new(page.pathname(), &search);
            browser.allow_writes(accepts_replace);
            let trace = Rc::new(Self {
                page,
                accepts_replace,
                browser: browser.clone(),
                directory: Cell::new(None),
                requests: RefCell::new(Vec::new()),
                second_page: Cell::new(None),
                release_page: Cell::new(false),
            });
            let mut dom = VirtualDom::new(ReadHarness)
                .with_root_context(trace.clone())
                .with_root_context(browser);
            dom.rebuild_in_place();
            settle(&mut dom);
            (dom, trace)
        }

        fn read_body(&self) -> Vec<String> {
            self.requests
                .borrow()
                .iter()
                .filter(|event| event.starts_with("body:"))
                .cloned()
                .collect()
        }
    }

    fn settle(dom: &mut VirtualDom) {
        for _ in 0..5 {
            dom.render_immediate_to_vec();
            dom.process_events();
        }
    }

    #[component]
    fn ReadHarness() -> Element {
        let control = consume_context::<Rc<ReadTrace>>();
        let trace = use_signal(|| control.clone());
        let query = use_page_query();
        let world = use_memo(|| Some("main".to_owned()));
        let refresh = use_signal(|| 0);
        let directory = use_round_directory_with(
            control.page,
            query,
            world,
            refresh,
            async move |world, status, page| {
                assert_eq!(world, "main");
                trace.peek().requests.borrow_mut().push(format!("{status}:{page}"));
                if status != "settled" {
                    return Ok((Vec::new(), true));
                }
                if page == 1 {
                    let complete = !trace.peek().accepts_replace;
                    return Ok((vec![auction(NEWEST, "main", 20, "settled", 4)], complete));
                }
                if page != 2 {
                    return pending().await;
                }
                poll_fn(|cx| {
                    let trace = trace.peek();
                    if trace.release_page.get() {
                        Poll::Ready(())
                    } else {
                        trace.second_page.set(Some(cx.waker().clone()));
                        Poll::Pending
                    }
                })
                .await;
                Ok((vec![auction(OLDER, "main", 2, "settled", 3)], true))
            },
        );
        let page = use_memo(|| 1);
        let size = use_memo(|| 20);
        let _ = use_round_body(
            directory,
            || Some(None),
            page,
            size,
            async move |id, world, lot, page, size| {
                assert_eq!((world.as_str(), lot, page, size), ("main", None, 1, 20));
                trace.peek().requests.borrow_mut().push(format!("body:{id}"));
                Ok(())
            },
        );
        control.directory.set(Some(directory));
        rsx! {}
    }

    #[test]
    fn default_body_starts_before_directory_options_need_a_second_page() {
        for page in [RoundPage::BidLog, RoundPage::Fulfillment] {
            let (mut dom, trace) = ReadTrace::mount(page, None, true);
            assert_eq!(trace.read_body(), [format!("body:{NEWEST}")]);
            assert!(!trace.requests.borrow().iter().any(|event| event == "settled:2"));
            if page == RoundPage::BidLog {
                let directory = trace.directory.get().unwrap();
                dom.in_runtime(|| directory.load_options());
                settle(&mut dom);
                assert!(trace.second_page.take().is_some(), "选择器需要补齐时才读取下一页");
                assert_eq!(trace.read_body(), [format!("body:{NEWEST}")]);
            }
        }
    }

    #[test]
    fn requested_round_does_not_read_the_body_until_its_own_page_is_confirmed() {
        let (mut dom, trace) = ReadTrace::mount(RoundPage::Fulfillment, Some(OLDER), true);
        assert_eq!(&*trace.requests.borrow(), &["settled:1", "settled:2"]);
        assert!(trace.read_body().is_empty());
        trace.release_page.set(true);
        trace.second_page.take().expect("第二页尚未完成").wake();
        settle(&mut dom);
        assert_eq!(trace.read_body(), [format!("body:{OLDER}")]);
        assert!(trace.browser.writes().is_empty());
    }

    #[test]
    fn a_rejected_single_round_replacement_keeps_the_manual_choice_available() {
        let (mut dom, trace) = ReadTrace::mount(RoundPage::Fulfillment, None, false);
        let directory = trace.directory.get().unwrap();
        let query = trace.browser.signal();
        dom.in_scope(ScopeId::APP, || {
            assert!(directory.correction_failed());
            assert!(directory.selected().is_none());
            assert_eq!(directory.options().len(), 1);
            assert!(trace.read_body().is_empty());
            assert_eq!(trace.browser.writes().len(), 1);
            assert!(!trace.browser.writes()[0].accepted);
            trace.browser.allow_writes(true);
            assert!(ReadSelection::Auction(NEWEST.to_owned()).select(query, trace.page.pathname()));
        });
        settle(&mut dom);
        assert_eq!(trace.read_body(), [format!("body:{NEWEST}")]);
        dom.in_runtime(|| assert!(!directory.correction_failed()));
    }
}
