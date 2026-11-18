//! 出价记录与履约进度共用的拍品筛选校验、草稿和地址提交。

use std::future::Future;

use auction_web_shared::timer::TimeoutFuture;
use auction_web_shared::validation::escaped_single_line_input;
use auction_web_shared::{QueryWriteMode, t};
use dioxus::prelude::*;
use uuid::Uuid;

use super::query::commit_read_query_with;
use crate::surface::{
    AuctionParameter, PublicBoardQuery, page_pathname, query_values, read_page_pairs,
    read_page_query, use_public_query_pairs, write_page_pairs,
};

/// 连续输入停顿后才应用筛选，合并读取请求。
const LOT_FILTER_WRITE_DEBOUNCE_MILLIS: u32 = 300;

/// 去空白后的 lot 筛选能否被公开竞价日志接口接受：空（不筛选）、不超过 32 个 ASCII 字母数字
/// 的编号，或规范小写连字符形式的非 nil UUID。口径与服务端一致，其余取值只会得到 400；
/// 履约进度接口沿用同一口径。
pub(super) fn valid_lot_filter(value: &str) -> bool {
    if value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return true;
    }
    Uuid::parse_str(value).is_ok_and(|lot_id| {
        let mut buffer = Uuid::encode_buffer();
        let canonical: &str = lot_id.hyphenated().encode_lower(&mut buffer);
        !lot_id.is_nil() && canonical == value
    })
}

#[derive(Clone, PartialEq)]
struct LotFilterLocation {
    world: crate::surface::FilterParameter,
    auction: AuctionParameter,
    lots: Vec<String>,
}

impl LotFilterLocation {
    fn new(query: &PublicBoardQuery, lots: Vec<String>) -> Self {
        Self {
            world: query
                .world
                .round_world()
                .map_or_else(|| query.world.clone(), |world| Some(world.to_owned()).into()),
            auction: query.auction_parameter().clone(),
            lots,
        }
    }

    #[cfg(test)]
    fn from_query(query: &PublicBoardQuery) -> Self {
        Self::new(query, query.bid_log_lot.iter().cloned().collect())
    }

    fn value(&self) -> &str {
        self.lots.first().map_or("", String::as_str)
    }

    fn requested_lot(&self) -> Result<Option<String>, LotFilterError> {
        match self.lots.as_slice() {
            [] => Ok(None),
            [raw] if !raw.trim().is_empty() && valid_lot_draft(raw) => {
                Ok(Some(raw.trim().to_owned()))
            }
            [_] => Err(LotFilterError::Invalid),
            _ => Err(LotFilterError::Duplicate),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LotFilterError {
    Invalid,
    Duplicate,
}

fn lot_values(pairs: &[(String, String)]) -> Vec<String> {
    query_values(pairs, "lot").map(str::to_owned).collect()
}

fn valid_lot_draft(value: &str) -> bool {
    !value.chars().any(char::is_control) && valid_lot_filter(value.trim())
}

/// 出价记录与履约进度共用的拍品筛选草稿和延时写入状态。
#[derive(Clone, Copy)]
pub(super) struct LotFilterDraft {
    query: Signal<PublicBoardQuery>,
    applied: Memo<LotFilterLocation>,
    value: Signal<String>,
    generation: Signal<u64>,
    edited: Signal<bool>,
    blurred: Signal<bool>,
    pathname: &'static str,
}

impl LotFilterDraft {
    /// 输入框正在编辑的值，保留原始空白。
    pub(super) fn value(self) -> String {
        (self.value)()
    }

    pub(super) fn applied_value(self) -> String {
        self.applied.read().value().to_owned()
    }

    pub(super) fn requested_lot(self) -> Result<Option<String>, LotFilterError> {
        self.applied.read().requested_lot()
    }

    pub(super) fn error(self, invalid_message: impl FnOnce() -> String) -> Option<String> {
        let error = if (self.edited)() {
            ((self.blurred)() && !valid_lot_draft(&self.value())).then_some(LotFilterError::Invalid)
        } else {
            self.requested_lot().err()
        }?;
        Some(match error {
            LotFilterError::Duplicate => {
                let values = self
                    .applied
                    .read()
                    .lots
                    .iter()
                    .map(|value| format!("{value:?}"))
                    .collect::<Vec<_>>()
                    .join(" / ");
                format!("{} {values}", t!("filter-error-duplicate"))
            }
            LotFilterError::Invalid => {
                let message = invalid_message();
                if let Some(value) = escaped_single_line_input(&self.value()) {
                    format!("{message} {value}")
                } else {
                    message
                }
            }
        })
    }

    pub(super) fn blur(mut self) {
        self.blurred.set(true);
    }

    /// 合法草稿等待输入停顿后再应用，非法草稿仅供字段提示。
    pub(super) fn edit(self, value: String) {
        if let Some(edit) = self.delayed_edit(
            value,
            || TimeoutFuture::new(LOT_FILTER_WRITE_DEBOUNCE_MILLIS),
            |pathname| {
                (page_pathname() == pathname)
                    .then(|| (read_page_query(), lot_values(&read_page_pairs())))
            },
            move |next, mode| write_lot_filter(next, self.pathname, mode),
        ) {
            spawn(edit);
        }
    }

    fn delayed_edit<F: Future<Output = ()> + 'static>(
        mut self,
        value: String,
        delay: impl FnOnce() -> F + 'static,
        read_query: impl FnOnce(&str) -> Option<(PublicBoardQuery, Vec<String>)> + 'static,
        write_query: impl FnOnce(&PublicBoardQuery, QueryWriteMode) -> bool + 'static,
    ) -> Option<impl Future<Output = ()> + 'static> {
        let lot = value.trim().to_owned();
        let valid = valid_lot_draft(&value);
        let generation = self.reset(value);
        self.edited.set(true);
        if !valid {
            return None;
        }
        let origin = self.applied.peek().clone();
        Some(async move {
            delay().await;
            if *self.generation.peek() != generation {
                return;
            }
            let Some((current, lots)) = read_query(self.pathname) else {
                return;
            };
            // 导航可能先于草稿同步。旧计时器不能覆盖另一世界、场次状态或筛选原文。
            let matches_applied = origin == *self.applied.peek();
            if matches_applied
                && origin == LotFilterLocation::new(&current, lots.clone())
                && !apply_lot_filter(self.query, current, &lots, lot, write_query)
            {
                self.restore(origin.value().to_owned());
            }
        })
    }

    /// 清除立即应用，并作废此前排队的输入。
    pub(super) fn clear(self) {
        if page_pathname() == self.pathname {
            self.clear_with(
                read_page_query(),
                &lot_values(&read_page_pairs()),
                move |next, mode| write_lot_filter(next, self.pathname, mode),
            );
        }
    }

    fn clear_with(
        mut self,
        current: PublicBoardQuery,
        lots: &[String],
        write_query: impl FnOnce(&PublicBoardQuery, QueryWriteMode) -> bool,
    ) {
        let written = apply_lot_filter(self.query, current, lots, String::new(), write_query);
        self.restore(if written {
            String::new()
        } else {
            lots.first().cloned().unwrap_or_default()
        });
    }

    fn restore(&mut self, value: String) {
        self.reset(value);
        self.edited.set(false);
    }

    fn reset(&mut self, value: String) -> u64 {
        let generation = self.generation.peek().wrapping_add(1);
        self.generation.set(generation);
        self.value.set(value);
        self.blurred.set(false);
        generation
    }
}

/// 已应用查询决定草稿的归属，翻页不改变归属。
pub(super) fn use_lot_filter_draft(
    query: Signal<PublicBoardQuery>,
    pathname: &'static str,
) -> LotFilterDraft {
    let pairs = use_public_query_pairs(query);
    use_lot_filter_state(query, pathname, move |query| {
        LotFilterLocation::new(query, lot_values(&pairs.read()))
    })
}

fn use_lot_filter_state(
    query: Signal<PublicBoardQuery>,
    pathname: &'static str,
    locate: impl Fn(&PublicBoardQuery) -> LotFilterLocation + 'static,
) -> LotFilterDraft {
    let applied = use_memo(move || locate(&query.read()));
    let mut previous = use_signal(|| applied.peek().clone());
    let mut draft = LotFilterDraft {
        query,
        applied,
        value: use_signal(|| applied.peek().value().to_owned()),
        generation: use_signal(|| 0_u64),
        edited: use_signal(|| false),
        blurred: use_signal(|| false),
        pathname,
    };
    use_effect(move || {
        let current = applied();
        let prior = previous.peek();
        if *prior == current {
            return;
        }
        let moved = prior.world != current.world || prior.auction != current.auction;
        drop(prior);
        // 清除非法场次也会作废旧草稿，重复拍品条件保留原文供修正。
        let should_restore = moved
            || current.requested_lot().is_err()
            || draft.value.peek().trim() != current.value();
        if should_restore {
            draft.restore(current.value().to_owned());
        }
        previous.set(current);
    });
    draft
}

fn write_lot_filter(query: &PublicBoardQuery, pathname: &str, mode: QueryWriteMode) -> bool {
    let pairs =
        query.bid_log_lot.iter().map(|lot| ("lot".to_owned(), lot.clone())).collect::<Vec<_>>();
    let keys: &[&str] =
        if pathname == "/bidlog" { &["lot", "page", "bidlog_page"] } else { &["lot", "page"] };
    write_page_pairs(keys, &pairs, mode)
}

/// 确认按现有值或成功写入的地址应用筛选。
fn apply_lot_filter(
    query_state: Signal<PublicBoardQuery>,
    mut next: PublicBoardQuery,
    applied: &[String],
    lot: String,
    write_query: impl FnOnce(&PublicBoardQuery, QueryWriteMode) -> bool,
) -> bool {
    let lot = (!lot.is_empty()).then_some(lot);
    if applied.iter().eq(lot.iter()) {
        return true;
    }
    next.bid_log_lot = lot;
    next.page = 1;
    next.bid_log_page_alias = false;
    commit_read_query_with(query_state, next, QueryWriteMode::Replace, write_query)
}
#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{Future, poll_fn, ready};
    use std::pin::pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    use auction_web_shared::QueryWriteMode;
    use auction_web_shared::query::QueryLocation;
    use dioxus::dioxus_core::{AttributeValue, Mutation, NoOpMutations};
    use dioxus::prelude::*;

    use super::{
        LOT_FILTER_WRITE_DEBOUNCE_MILLIS, LotFilterDraft, LotFilterError, LotFilterLocation,
        use_lot_filter_state, valid_lot_filter,
    };
    use crate::surface::reads::query::ReadSelection;
    use crate::surface::reads::query::tests::{OLDER_ROUND_ID, ROUND_ID};
    use crate::surface::{PublicBoardQuery, read_public_query};

    #[derive(Default)]
    struct LotDraftControl {
        query: Cell<Option<Signal<PublicBoardQuery>>>,
        draft: Cell<Option<LotFilterDraft>>,
    }

    #[component]
    fn LotDraftHarness() -> Element {
        let query = use_signal(|| {
            PublicBoardQuery { page: 4, ..Default::default() }
                .with_auction_values([ROUND_ID.to_owned()])
        });
        let draft = use_lot_filter_state(query, "/bidlog", LotFilterLocation::from_query);
        let control = consume_context::<Rc<LotDraftControl>>();
        control.query.set(Some(query));
        control.draft.set(Some(draft));
        let applied = query.read();
        rsx! {
            select { name: "world", value: applied.world.as_deref().unwrap_or("main").to_owned() }
            select { name: "auction", value: applied.auction_id().unwrap_or_default() }
        }
    }

    fn mounted_filter() -> (VirtualDom, Signal<PublicBoardQuery>, LotFilterDraft) {
        let control = Rc::new(LotDraftControl::default());
        let mut dom = VirtualDom::new(LotDraftHarness).with_root_context(control.clone());
        dom.rebuild_in_place();
        dom.process_events();
        let query = control.query.get().expect("挂载查询信号");
        let draft = control.draft.get().expect("挂载拍品筛选草稿");
        (dom, query, draft)
    }

    fn settle_query_change(dom: &mut VirtualDom) {
        // 先提交查询触发的渲染，再让低优先级的草稿同步 effect 运行。
        dom.render_immediate(&mut NoOpMutations);
        dom.process_events();
    }

    #[test]
    fn applied_auction_and_world_changes_discard_the_draft_even_with_the_same_lot_filter() {
        let (mut dom, mut query, mut draft) = mounted_filter();

        for (world, auction, applied_lot) in [
            (None, OLDER_ROUND_ID, None),
            (Some("isekai"), OLDER_ROUND_ID, None),
            (Some("isekai"), ROUND_ID, Some("One3")),
        ] {
            let previous = dom.in_runtime(|| draft.reset("One12".to_owned()));
            dom.in_runtime(|| query.write().page = 2);
            settle_query_change(&mut dom);
            assert_eq!(draft.value.peek().as_str(), "One12");
            assert_eq!(*draft.generation.peek(), previous);

            dom.in_runtime(|| {
                let next = PublicBoardQuery {
                    world: world.map(str::to_owned).into(),
                    bid_log_lot: applied_lot.map(str::to_owned),
                    ..query.peek().clone()
                };
                query.set(next.with_auction_values([auction.to_owned()]));
            });
            settle_query_change(&mut dom);
            assert_eq!(draft.value.peek().as_str(), applied_lot.unwrap_or_default());
            assert_ne!(*draft.generation.peek(), previous);
        }
    }

    struct History {
        query: RefCell<PublicBoardQuery>,
        accepts_write: Cell<bool>,
        write_attempts: Cell<u32>,
    }

    impl History {
        fn new(query: PublicBoardQuery, accepts_write: bool) -> Rc<Self> {
            Rc::new(Self {
                query: RefCell::new(query),
                accepts_write: Cell::new(accepts_write),
                write_attempts: Cell::new(0),
            })
        }

        fn write(&self, query: &PublicBoardQuery, mode: QueryWriteMode) -> bool {
            assert!(matches!(mode, QueryWriteMode::Replace));
            self.try_write(query)
        }

        fn try_write(&self, query: &PublicBoardQuery) -> bool {
            self.write_attempts.set(self.write_attempts.get() + 1);
            if self.accepts_write.get() {
                self.query.replace(query.clone());
                return true;
            }
            false
        }
    }

    fn input<F: Future<Output = ()> + 'static>(
        draft: LotFilterDraft,
        value: &str,
        history: &Rc<History>,
        delay: impl FnOnce() -> F + 'static,
    ) -> impl Future<Output = ()> + 'static {
        let reader = Rc::clone(history);
        let writer = Rc::clone(history);
        draft
            .delayed_edit(
                value.to_owned(),
                delay,
                move |path| {
                    assert_eq!(path, "/bidlog");
                    let current = reader.query.borrow().clone();
                    let values = LotFilterLocation::from_query(&current).lots;
                    Some((current, values))
                },
                move |next, mode| writer.write(next, mode),
            )
            .expect("合法草稿安排防抖写入")
    }

    fn submit_input(dom: &VirtualDom, draft: LotFilterDraft, value: &str, history: &Rc<History>) {
        dom.in_runtime(|| {
            let mut edit = pin!(input(draft, value, history, || ready(())));
            assert_eq!(
                edit.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(())
            );
        });
    }

    fn select_target(
        dom: &VirtualDom,
        query: Signal<PublicBoardQuery>,
        selection: ReadSelection,
        history: &History,
    ) -> bool {
        let address = history.query.borrow().clone();
        dom.in_scope(ScopeId::APP, || {
            selection.select_with(query, address, |next, mode| {
                assert!(matches!(mode, QueryWriteMode::Push));
                history.try_write(next)
            })
        })
    }

    fn check_rejected_selection(
        selection: ReadSelection,
        selected_world: Option<&str>,
        selected_auction: Option<&str>,
    ) {
        let (mut dom, mut query, draft) = mounted_filter();
        let initial =
            PublicBoardQuery { bid_log_lot: Some("One7".to_owned()), ..query.peek().clone() };
        dom.in_runtime(|| query.set(initial.clone()));
        settle_query_change(&mut dom);
        assert_eq!(draft.value.peek().as_str(), "One7");
        let generation = *draft.generation.peek();
        let history = History::new(initial.clone(), false);

        assert!(!select_target(&dom, query, selection.clone(), &history));
        assert_eq!(&*history.query.borrow(), &initial);
        let redraw = dom.render_immediate_to_vec();
        let after_refusal = query.peek().clone();
        let draft_after_refusal = draft.value.peek().clone();
        let generation_after_refusal = *draft.generation.peek();

        history.accepts_write.set(true);
        submit_input(&dom, draft, "One12", &history);
        assert_eq!(history.write_attempts.get(), 2, "拒写后原场次仍可提交拍品筛选");
        assert_eq!(after_refusal, initial);
        assert_eq!(draft_after_refusal, "One7");
        assert_eq!(generation_after_refusal, generation);
        for expected in ["main", ROUND_ID] {
            assert!(redraw.edits.iter().any(|edit| matches!(
                edit,
                Mutation::SetAttribute { name: "value", value: AttributeValue::Text(value), .. }
                    if value == expected
            )));
        }
        assert_eq!(query.peek().world, initial.world);
        assert_eq!(query.peek().auction_id(), initial.auction_id());
        assert_eq!(query.peek().bid_log_lot.as_deref(), Some("One12"));
        assert_eq!(&*query.peek(), &*history.query.borrow());

        let address = history.query.borrow().clone();
        dom.in_runtime(|| {
            draft.clear_with(
                address.clone(),
                &LotFilterLocation::from_query(&address).lots,
                |next, mode| history.write(next, mode),
            );
        });
        assert_eq!(history.write_attempts.get(), 3);
        assert!(query.peek().bid_log_lot.is_none());
        assert_eq!(query.peek().auction_id(), initial.auction_id());
        assert_eq!(query.peek().world, initial.world);
        assert!(draft.value.peek().is_empty());
        assert_eq!(&*query.peek(), &*history.query.borrow());

        assert!(select_target(&dom, query, selection, &history));
        assert_eq!(history.write_attempts.get(), 4);
        assert_eq!(query.peek().world.as_deref(), selected_world);
        assert_eq!(query.peek().auction_id(), selected_auction);
        assert_eq!(&*query.peek(), &*history.query.borrow());
    }

    #[test]
    fn rejected_auction_selection_leaves_the_original_lot_filter_usable() {
        check_rejected_selection(
            ReadSelection::Auction(OLDER_ROUND_ID.to_owned()),
            None,
            Some(OLDER_ROUND_ID),
        );
    }

    #[test]
    fn rejected_world_selection_leaves_the_original_lot_filter_usable() {
        check_rejected_selection(ReadSelection::World("isekai".to_owned()), Some("isekai"), None);
    }

    #[test]
    fn rejected_address_write_leaves_the_query_usable_for_the_next_input() {
        let (dom, query, draft) = mounted_filter();
        let initial = query.peek().clone();
        let history = History::new(initial.clone(), false);

        submit_input(&dom, draft, "One12", &history);
        assert_eq!(history.write_attempts.get(), 1);
        assert_eq!(&*history.query.borrow(), &initial);
        assert_eq!(&*query.peek(), &initial);
        assert!(draft.value.peek().is_empty());

        history.accepts_write.set(true);
        submit_input(&dom, draft, "One12", &history);
        assert_eq!(history.write_attempts.get(), 2);
        assert_eq!(query.peek().bid_log_lot.as_deref(), Some("One12"));
        assert_eq!(query.peek().page, 1);
        assert_eq!(&*query.peek(), &*history.query.borrow());
    }

    #[test]
    fn clearing_after_a_rejected_write_restores_the_applied_empty_filter() {
        let (dom, query, draft) = mounted_filter();
        let initial = query.peek().clone();
        let history = History::new(initial.clone(), false);
        submit_input(&dom, draft, "One12", &history);
        assert!(draft.value.peek().is_empty());

        let current = history.query.borrow().clone();
        dom.in_runtime(|| {
            draft.clear_with(
                current.clone(),
                &LotFilterLocation::from_query(&current).lots,
                |next, mode| history.write(next, mode),
            );
        });

        assert_eq!(history.write_attempts.get(), 1);
        assert_eq!(&*query.peek(), &initial);
        assert_eq!(&*history.query.borrow(), &initial);
        assert!(draft.value.peek().is_empty());
    }

    #[test]
    fn rejected_clear_preserves_the_applied_filter_and_draft_until_retry() {
        let (mut dom, query, draft) = mounted_filter();
        let history = History::new(query.peek().clone(), true);
        submit_input(&dom, draft, "One12", &history);
        dom.render_immediate(&mut NoOpMutations);
        let initial = query.peek().clone();

        history.accepts_write.set(false);
        let current = history.query.borrow().clone();
        dom.in_runtime(|| {
            draft.clear_with(
                current.clone(),
                &LotFilterLocation::from_query(&current).lots,
                |next, mode| history.write(next, mode),
            );
        });
        assert_eq!(history.write_attempts.get(), 2);
        assert_eq!(&*query.peek(), &initial);
        assert_eq!(&*history.query.borrow(), &initial);
        assert_eq!(draft.value.peek().as_str(), "One12");

        submit_input(&dom, draft, "", &history);
        assert_eq!(history.write_attempts.get(), 3);
        assert_eq!(&*query.peek(), &initial);
        assert_eq!(draft.value.peek().as_str(), "One12");

        history.accepts_write.set(true);
        let current = history.query.borrow().clone();
        dom.in_runtime(|| {
            draft.clear_with(
                current.clone(),
                &LotFilterLocation::from_query(&current).lots,
                |next, mode| history.write(next, mode),
            );
        });
        assert_eq!(history.write_attempts.get(), 4);
        assert!(query.peek().bid_log_lot.is_none());
        assert_eq!(&*query.peek(), &*history.query.borrow());
        assert!(draft.value.peek().is_empty());
    }

    #[test]
    fn timer_expiring_after_same_path_back_navigation_never_replaces_the_address() {
        let (dom, query, draft) = mounted_filter();
        let initial = query.peek().clone();
        let history = History::new(initial.clone(), true);
        let clock = Rc::new(Cell::new(0));
        let elapsed = Rc::clone(&clock);
        let mut edit = pin!(dom.in_runtime(|| {
            input(draft, "One12", &history, move || {
                poll_fn(move |_| {
                    if elapsed.get() >= LOT_FILTER_WRITE_DEBOUNCE_MILLIS {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
            })
        }));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(dom.in_runtime(|| edit.as_mut().poll(&mut context)), Poll::Pending);
        assert_eq!(draft.value.peek().as_str(), "One12");
        let previous = *draft.generation.peek();
        let restored = initial.clone().with_auction_values([OLDER_ROUND_ID.to_owned()]);
        // 浏览器已返回另一场，组件的查询和草稿代次尚待同步。
        history.query.replace(restored.clone());
        clock.set(LOT_FILTER_WRITE_DEBOUNCE_MILLIS);
        assert_eq!(dom.in_runtime(|| edit.as_mut().poll(&mut context)), Poll::Ready(()));

        assert_eq!(*draft.generation.peek(), previous);
        assert_eq!(history.write_attempts.get(), 0);
        assert_eq!(&*history.query.borrow(), &restored);
        assert_eq!(&*query.peek(), &initial);
    }

    #[test]
    fn recovering_from_an_invalid_auction_discards_the_lot_timer_before_directory_confirmation() {
        struct InvalidLink;

        impl QueryLocation for InvalidLink {
            fn current(&self) -> String {
                "/bidlog?auction=bad".to_owned()
            }

            fn write(&self, _location: &str, _mode: QueryWriteMode) -> bool {
                false
            }
        }

        let (mut dom, mut query, draft) = mounted_filter();
        let initial = dom.in_scope(ScopeId::APP, || {
            provide_context(Rc::new(InvalidLink) as Rc<dyn QueryLocation>);
            let initial = read_public_query();
            query.set(initial.clone());
            initial
        });
        settle_query_change(&mut dom);
        let history = History::new(initial, true);
        let expired = Rc::new(Cell::new(false));
        let timer = expired.clone();
        let mut edit = pin!(dom.in_runtime(|| {
            input(draft, "One12", &history, move || {
                poll_fn(move |_| if timer.get() { Poll::Ready(()) } else { Poll::Pending })
            })
        }));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(dom.in_runtime(|| edit.as_mut().poll(&mut context)), Poll::Pending);

        assert!(select_target(&dom, query, ReadSelection::Clear, &history));
        // 场次目录尚未确认，地址已经清除了非法场次。先让旧计时器到期，再同步草稿。
        expired.set(true);
        assert_eq!(dom.in_runtime(|| edit.as_mut().poll(&mut context)), Poll::Ready(()));
        assert_eq!(history.write_attempts.get(), 1);
        assert_eq!(&*history.query.borrow(), &PublicBoardQuery::default());
        settle_query_change(&mut dom);
        assert!(draft.value.peek().is_empty());
    }

    #[test]
    fn delayed_lot_write_checks_the_applied_query_even_before_the_draft_syncs() {
        let matches_query = |origin: &LotFilterLocation, query: &PublicBoardQuery| {
            *origin == LotFilterLocation::from_query(query)
        };
        let initial = PublicBoardQuery::default().with_auction_values([ROUND_ID.to_owned()]);
        let origin = LotFilterLocation::from_query(&initial);
        assert!(matches_query(
            &origin,
            &PublicBoardQuery { world: Some("main".to_owned()).into(), page: 2, ..initial.clone() }
        ));
        assert!(!matches_query(
            &origin,
            &PublicBoardQuery { world: Some("isekai".to_owned()).into(), ..initial.clone() }
        ));
        assert!(!matches_query(
            &origin,
            &initial.clone().with_auction_values([OLDER_ROUND_ID.to_owned()])
        ));
        assert!(!matches_query(
            &origin,
            &PublicBoardQuery { bid_log_lot: Some("One3".to_owned()), ..initial }
        ));
    }

    #[test]
    fn duplicate_empty_and_invalid_link_filters_block_reads_until_replaced() {
        let query = PublicBoardQuery::default();
        let duplicates = vec!["One12".to_owned(), "Sta1".to_owned()];
        let duplicate = LotFilterLocation::new(&query, duplicates.clone());
        assert_eq!(duplicate.lots, duplicates);
        assert_eq!(duplicate.requested_lot(), Err(LotFilterError::Duplicate));
        for value in ["", " ", "One-12", "One12\n"] {
            let invalid = LotFilterLocation::new(&query, vec![value.to_owned()]);
            assert_eq!(invalid.value(), value);
            assert_eq!(invalid.requested_lot(), Err(LotFilterError::Invalid));
        }
        assert_eq!(LotFilterLocation::new(&query, Vec::new()).requested_lot(), Ok(None));
        assert_eq!(
            LotFilterLocation::new(&query, vec!["Sta1".to_owned()]).requested_lot(),
            Ok(Some("Sta1".to_owned()))
        );
    }

    #[test]
    fn lot_filter_accepts_what_the_bid_log_endpoint_accepts() {
        assert!(valid_lot_filter(""));
        assert!(valid_lot_filter("One12"));
        assert!(valid_lot_filter(&"a".repeat(32)));
        assert!(valid_lot_filter("67e55044-10b1-426f-9247-bb680e5fe0c8"));
    }

    #[test]
    fn lot_filter_rejects_values_the_endpoint_answers_with_400() {
        assert!(!valid_lot_filter(&"a".repeat(33)));
        assert!(!valid_lot_filter("One 12"));
        assert!(!valid_lot_filter("One-12"));
        assert!(!valid_lot_filter("拍品12"));
        // 地址栏原样保留的超长或含控制字符的值同样落到字段错误，不发请求。
        assert!(!valid_lot_filter(&"a".repeat(129)));
        assert!(!valid_lot_filter("One\u{7}12"));
        // 服务端只接受规范小写连字符形式的非 nil UUID。
        assert!(!valid_lot_filter("67E55044-10B1-426F-9247-BB680E5FE0C8"));
        assert!(!valid_lot_filter("{67e55044-10b1-426f-9247-bb680e5fe0c8}"));
        assert!(!valid_lot_filter("00000000-0000-0000-0000-000000000000"));
    }
}
