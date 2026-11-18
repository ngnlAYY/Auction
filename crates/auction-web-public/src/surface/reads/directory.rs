//! 竞价日志与履约选择器的分批目录、场次确认及地址纠偏。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use auction_web_shared::{QueryWriteMode, t};
use dioxus::dioxus_core::Task;
use dioxus::prelude::*;

use crate::api::{HistoricalAuction, fetch_bid_log_auctions, fetch_fulfillment_auctions};
use crate::surface::{PublicBoardQuery, commit_public_query_pairs, page_pathname, read_page_query};

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
    world: String,
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

    fn resolve(&self, requested_id: &str) -> Resolution<'_> {
        if let Some(found) = self.options.iter().find(|auction| auction.id == requested_id) {
            return Resolution::Confirmed(Some(found));
        }
        // 同一世界内 API 按期号和 ID 降序；默认场次只需比较各状态首页。
        // 指定场次未找到时则必须查实目录结束，不能把未读取的场次判成不可见。
        let incomplete = |status: &&StatusPages| {
            if requested_id.is_empty() {
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

    fn requests(&self, requested_id: &str, load_options: bool) -> Vec<(&'static str, i64)> {
        let seek =
            !requested_id.is_empty() && matches!(self.resolve(requested_id), Resolution::Waiting);
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
    expected: String,
    next: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) struct RoundDirectory {
    view: Memo<Option<DirectoryView>>,
    confirmed: Memo<Option<ConfirmedRound>>,
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
        self.opened_world.set(Some(self.scope.peek().world.clone()));
    }
}

/// 目录逐页确认场次。选定后暂停翻页，打开选择器时再补齐剩余选项。
pub(super) fn use_round_directory(
    page: RoundPage,
    query: Signal<PublicBoardQuery>,
    world: Memo<String>,
    refresh: Signal<u64>,
) -> RoundDirectory {
    use_round_directory_with(page, query, world, refresh, fetch_bid_log_auctions, replace_round)
}

fn use_round_directory_with(
    page: RoundPage,
    query: Signal<PublicBoardQuery>,
    world: Memo<String>,
    refresh: Signal<u64>,
    fetch_page: impl AsyncFn(
        String,
        &'static str,
        i64,
    ) -> Result<(Vec<HistoricalAuction>, bool), String>
    + Clone
    + 'static,
    replace: impl Fn(Signal<PublicBoardQuery>, RoundPage, &RoundCorrection) -> Option<bool> + 'static,
) -> RoundDirectory {
    let scope = use_memo(move || DirectoryScope { world: world(), generation: refresh() });
    let requested = use_memo(move || query.read().auction_id.clone().unwrap_or_default());
    let mut directory = use_signal(|| Directory::new(scope.peek().clone(), page, Rc::from([])));
    let mut opened_world = use_signal(|| None::<String>);
    let tasks = use_hook(|| Rc::new(RefCell::new(BTreeMap::<&str, Task>::new())));
    let full_fulfillment_request = use_memo(move || {
        let scope = scope();
        let current = directory.read();
        (page == RoundPage::Fulfillment
            && current.scope == scope
            && current.needs_more_options()
            && opened_world.read().as_deref() == Some(scope.world.as_str()))
        .then_some(scope)
    });
    let full_fulfillment = use_resource(move || {
        let scope = full_fulfillment_request();
        async move {
            let scope = scope?;
            let result = fetch_fulfillment_auctions(scope.world.clone()).await.map(Rc::<[_]>::from);
            Some((scope, result))
        }
    });
    use_effect(move || {
        let current = scope();
        let wanted = requested();
        let opened = opened_world.read().as_deref() == Some(current.world.as_str());
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
        if full_fulfillment
            .read_unchecked()
            .as_ref()
            .and_then(Option::as_ref)
            .is_some_and(|(scope, result)| *scope == current && result.is_ok())
        {
            return;
        }
        let requests = directory.read().requests(&wanted, opened && page == RoundPage::BidLog);
        for (status, number) in requests {
            if let Some(progress) =
                directory.write().statuses.iter_mut().find(|entry| entry.status == status)
            {
                progress.running = true;
            }
            let expected = current.clone();
            let fetch_page = fetch_page.clone();
            let task = spawn(async move {
                let result = fetch_page(expected.world.clone(), status, number).await;
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
        let id = requested();
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
                        full.iter().find(|auction| auction.id == id).or_else(|| full.first()),
                    ),
                    full.clone(),
                    false,
                )
            } else {
                let failed = full.is_some_and(|(_, result)| result.is_err())
                    || current.statuses.iter().any(|page| page.failed);
                let resolution = current.resolve(&id);
                let rows = if matches!(resolution, Resolution::Waiting)
                    && current.previous_options.iter().any(|auction| auction.id == id)
                    && !current.options.iter().any(|auction| auction.id == id)
                {
                    current.previous_options.clone()
                } else {
                    current.display_options()
                };
                (resolution, rows, failed)
            }
        };
        let (readable, correction, empty, failed) = match resolution {
            Resolution::Confirmed(choice) => {
                let next = choice.map(|auction| auction.id.clone());
                let mut readable = None;
                let mut correction = None;
                if next.as_deref().unwrap_or_default() != id {
                    correction =
                        Some(RoundCorrection { scope: scope.clone(), expected: id.clone(), next });
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
        let selected = option_rows.iter().find(|auction| auction.id == id).cloned();
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
            let written = replace(query, page, &change);
            rejected.set(written == Some(false));
        }
    });
    let confirmed = use_memo(move || view.read().as_ref().and_then(|value| value.readable.clone()));
    RoundDirectory { view, confirmed, rejected, opened_world, scope }
}

fn replace_round(
    query: Signal<PublicBoardQuery>,
    page: RoundPage,
    change: &RoundCorrection,
) -> Option<bool> {
    if page_pathname() != page.pathname() {
        return None;
    }
    let current = read_page_query();
    let same_world = current.world == query.peek().world;
    if !same_world {
        return None;
    }
    correct_round(change, current.auction_id.as_deref(), page, |keys, pairs| {
        commit_public_query_pairs(query, keys, pairs, QueryWriteMode::Replace)
    })
}

fn correct_round(
    change: &RoundCorrection,
    actual: Option<&str>,
    page: RoundPage,
    write: impl FnOnce(&[&str], &[(String, String)]) -> bool,
) -> Option<bool> {
    if actual.unwrap_or_default() != change.expected {
        return None;
    }
    let keys: &[&str] = if page == RoundPage::BidLog {
        &["auction", "page", "bidlog_page"]
    } else {
        &["auction", "page"]
    };
    let pairs = change.next.iter().map(|id| ("auction".to_owned(), id.clone())).collect::<Vec<_>>();
    // 必须先写入已确认的场次才允许 GET，纠偏也不能把非法 lot 当成未提供。
    Some(write(keys, &pairs))
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

    use auction_web_shared::QueryWriteMode;
    use dioxus::prelude::*;

    use super::{
        RoundDirectory, RoundPage, correct_round, order_bid_log_auctions, use_round_directory_with,
    };
    use crate::api::HistoricalAuction;
    use crate::surface::PublicBoardQuery;
    use crate::surface::reads::query::{ReadSelection, use_round_body};

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
        selected: Option<&'static str>,
        accepts_replace: bool,
        query: Cell<Option<Signal<PublicBoardQuery>>>,
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
            let trace = Rc::new(Self {
                page,
                selected,
                accepts_replace,
                query: Cell::new(None),
                directory: Cell::new(None),
                requests: RefCell::new(Vec::new()),
                second_page: Cell::new(None),
                release_page: Cell::new(false),
            });
            let mut dom = VirtualDom::new(ReadHarness).with_root_context(trace.clone());
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
        let query = use_signal(|| PublicBoardQuery {
            auction_id: control.selected.map(str::to_owned),
            ..Default::default()
        });
        let world = use_memo(|| "main".to_owned());
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
                    return Ok((vec![auction("newest", "main", 20, "settled", 4)], complete));
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
                Ok((vec![auction("older", "main", 2, "settled", 3)], true))
            },
            move |mut state, page, change| {
                let actual = state.peek().auction_id.clone();
                correct_round(change, actual.as_deref(), page, |keys, pairs| {
                    let trace = trace.peek();
                    assert_eq!(keys[..2], ["auction", "page"]);
                    trace.requests.borrow_mut().push("replace".to_owned());
                    if trace.accepts_replace {
                        state.write().auction_id = pairs.first().map(|(_, id)| id.clone());
                    }
                    trace.accepts_replace
                })
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
        control.query.set(Some(query));
        control.directory.set(Some(directory));
        rsx! {}
    }

    #[test]
    fn default_body_starts_before_directory_options_need_a_second_page() {
        for page in [RoundPage::BidLog, RoundPage::Fulfillment] {
            let (mut dom, trace) = ReadTrace::mount(page, None, true);
            assert_eq!(trace.read_body(), ["body:newest"]);
            assert!(!trace.requests.borrow().iter().any(|event| event == "settled:2"));
            if page == RoundPage::BidLog {
                let directory = trace.directory.get().unwrap();
                dom.in_runtime(|| directory.load_options());
                settle(&mut dom);
                assert!(trace.second_page.take().is_some(), "选择器需要补齐时才读取下一页");
                assert_eq!(trace.read_body(), ["body:newest"]);
            }
        }
    }

    #[test]
    fn requested_round_does_not_read_the_body_until_its_own_page_is_confirmed() {
        let (mut dom, trace) = ReadTrace::mount(RoundPage::Fulfillment, Some("older"), true);
        assert_eq!(&*trace.requests.borrow(), &["settled:1", "settled:2"]);
        assert!(trace.read_body().is_empty());
        trace.release_page.set(true);
        trace.second_page.take().expect("第二页尚未完成").wake();
        settle(&mut dom);
        assert_eq!(trace.read_body(), ["body:older"]);
        assert!(!trace.requests.borrow().iter().any(|event| event == "replace"));
    }

    #[test]
    fn a_rejected_single_round_replacement_keeps_the_manual_choice_available() {
        let (mut dom, trace) = ReadTrace::mount(RoundPage::Fulfillment, None, false);
        let directory = trace.directory.get().unwrap();
        let query = trace.query.get().unwrap();
        dom.in_runtime(|| {
            assert!(directory.correction_failed());
            assert!(directory.selected().is_none());
            assert_eq!(directory.options().len(), 1);
            assert!(trace.read_body().is_empty());
            let address = query.peek().clone();
            assert!(ReadSelection::Auction("newest".to_owned()).select_with(
                query,
                address,
                |next, mode| {
                    assert_eq!(mode, QueryWriteMode::Push);
                    assert_eq!(next.auction_id.as_deref(), Some("newest"));
                    true
                }
            ));
        });
        settle(&mut dom);
        assert_eq!(trace.read_body(), ["body:newest"]);
        dom.in_runtime(|| assert!(!directory.correction_failed()));
    }
}
