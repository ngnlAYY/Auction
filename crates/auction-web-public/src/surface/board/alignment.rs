//! 看板目录与当前场次的对齐重读调度：同一上下文只自动重读一次，读取在途先等待，两侧都来自那次
//! 重读仍不对齐才判定用尽。纯判定函数与持有尝试记录、发起失效重读的 hook 放在一起，由
//! `AuctionData` 调用；组件调度测试用可控的读取替身驱动真实 `Resource`、父组件读取状态与两个刷新
//! 代号。

use dioxus::prelude::*;

use super::refresh::refresh_public_reads;

/// 一次渲染的对齐重读调度结果。
pub(super) struct AlignmentReread {
    /// 自动对齐重读已用尽：快照仍不对齐，且场次与目录都已来自那次重读。
    pub(super) exhausted: bool,
    /// 同一上下文的重读记录（上下文键与目录刷新代号）；用尽面板的重试清掉它，由调度重新发起一次。
    pub(super) attempt: Signal<Option<(String, u64)>>,
}

/// 对齐重读调度 hook。同一上下文只自动对齐重读一次，记录上下文键与重读使用的目录刷新代号；
/// 对齐重读与用尽面板的重试都经共用刷新入口：先失效公开读缓存，再同时重读当前（或历史）场次与
/// 目录，任一侧滞后都能追上，不会命中 TTL 内的新鲜条目而原样返回同一对不一致的快照。
/// `reading` 必须以订阅方式读取场次与目录资源状态，读取结束才会重新判定。
pub(super) fn use_alignment_reread(
    aligned: bool,
    alignment_key: String,
    loaded_generation: Option<u64>,
    reading: bool,
    mut lots_refresh_generation: Signal<u64>,
    refresh_generation: Signal<u64>,
) -> AlignmentReread {
    let mut alignment_attempt = use_signal(|| None::<(String, u64)>);
    use_effect(use_reactive(
        (&aligned, &alignment_key, &loaded_generation, &reading),
        move |(aligned, alignment_key, loaded_generation, reading)| {
            let attempt = alignment_attempt();
            match plan_alignment_reread(
                aligned,
                &alignment_key,
                attempt.as_ref(),
                loaded_generation,
                reading,
            ) {
                AlignmentAction::Wait => {}
                AlignmentAction::Clear => alignment_attempt.set(None),
                AlignmentAction::Reread => {
                    let next_generation = lots_refresh_generation.peek().saturating_add(1);
                    alignment_attempt.set(Some((alignment_key, next_generation)));
                    lots_refresh_generation.set(next_generation);
                    refresh_public_reads(refresh_generation);
                }
            }
        },
    ));
    let exhausted = alignment_retry_exhausted(
        aligned,
        &alignment_key,
        alignment_attempt.read().as_ref(),
        loaded_generation,
        reading,
    );
    AlignmentReread { exhausted, attempt: alignment_attempt }
}

/// 目录与当前场次对齐判定后的自动重读动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlignmentAction {
    /// 无需动作：已对齐且没有尝试记录，仍有读取在途，本上下文已重读过，或上一轮重读尚未返回。
    Wait,
    /// 已对齐：清掉尝试记录，之后再不对齐时重新获得一次自动重读。
    Clear,
    /// 为当前上下文发起一次失效后的对齐重读。
    Reread,
}

/// 纯判定函数：同一上下文（场次、阶段、开放状态与预览修订号）只自动重读一次。场次或目录仍有
/// 读取在途时先等待：返回后可能就对齐，此时重读只会取消在途读取。上一轮为另一上下文发起的重读
/// 还没返回（目录快照早于那次代号）时同样等待，避免场次读取在一轮之内改变上下文而反复刷新。
fn plan_alignment_reread(
    aligned: bool,
    alignment_key: &str,
    attempt: Option<&(String, u64)>,
    loaded_generation: Option<u64>,
    reading: bool,
) -> AlignmentAction {
    if aligned {
        return if attempt.is_some() { AlignmentAction::Clear } else { AlignmentAction::Wait };
    }
    if reading {
        return AlignmentAction::Wait;
    }
    match attempt {
        Some((key, _)) if key == alignment_key => AlignmentAction::Wait,
        Some((_, generation)) if loaded_generation.is_none_or(|loaded| loaded < *generation) => {
            AlignmentAction::Wait
        }
        _ => AlignmentAction::Reread,
    }
}

/// 目录快照与当前场次不对齐，且场次与目录都已来自为同一上下文发起的那次对齐重读：自动重读已
/// 用尽。重读刚排定、目录资源仍持有旧快照，或场次、目录仍有读取在途时都不算用尽：任一侧返回
/// 后都可能对齐，避免错误面板一闪而过。
fn alignment_retry_exhausted(
    aligned: bool,
    alignment_key: &str,
    attempt: Option<&(String, u64)>,
    loaded_generation: Option<u64>,
    reading: bool,
) -> bool {
    !aligned
        && !reading
        && attempt.is_some_and(|(key, attempt_generation)| {
            key == alignment_key
                && loaded_generation.is_some_and(|generation| generation >= *attempt_generation)
        })
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    use dioxus::dioxus_core::NoOpMutations;
    use dioxus::prelude::*;

    use super::{
        AlignmentAction, alignment_retry_exhausted, plan_alignment_reread, use_alignment_reread,
    };
    use crate::surface::board::refresh::refresh_public_reads;

    #[test]
    fn alignment_is_exhausted_only_after_the_retry_snapshot_arrives() {
        let attempt = ("auction:live".to_owned(), 3);
        // 对齐时从不算用尽。
        assert!(!alignment_retry_exhausted(true, "auction:live", Some(&attempt), Some(3), false));
        // 尚未发起对齐重读。
        assert!(!alignment_retry_exhausted(false, "auction:live", None, Some(2), false));
        // 重读已排定，但资源仍持有排定前的旧快照：不闪错误面板。
        assert!(!alignment_retry_exhausted(false, "auction:live", Some(&attempt), Some(2), false));
        // 读取失败由错误分支处理，这里没有已加载的代号。
        assert!(!alignment_retry_exhausted(false, "auction:live", Some(&attempt), None, false));
        // 上下文已切换：旧尝试不算数，等待 effect 为新上下文重读。
        assert!(!alignment_retry_exhausted(false, "auction:final", Some(&attempt), Some(3), false));
        // 那次对齐重读的快照仍不对齐（例如 503 时缓存回退的旧快照）：用尽，显示可重试的错误。
        assert!(alignment_retry_exhausted(false, "auction:live", Some(&attempt), Some(3), false));
        assert!(alignment_retry_exhausted(false, "auction:live", Some(&attempt), Some(4), false));
    }

    #[test]
    fn exhaustion_also_waits_for_reads_still_in_flight() {
        // 目录已按本轮代号返回，但当前（或历史深链的）场次或目录仍有读取在途：返回后可能就对齐，
        // 不算用尽。
        let attempt = ("auction:preview:3".to_owned(), 5);
        assert!(!alignment_retry_exhausted(
            false,
            "auction:preview:3",
            Some(&attempt),
            Some(5),
            true
        ));
        assert!(alignment_retry_exhausted(
            false,
            "auction:preview:3",
            Some(&attempt),
            Some(5),
            false
        ));
    }

    #[test]
    fn each_misaligned_context_gets_exactly_one_reread() {
        // 已对齐：没有尝试时无需动作；有尝试时清掉，之后再不对齐重新获得一次重读。
        assert_eq!(
            plan_alignment_reread(true, "a:live", None, Some(1), false),
            AlignmentAction::Wait
        );
        let attempt = ("a:live".to_owned(), 2);
        assert_eq!(
            plan_alignment_reread(true, "a:live", Some(&attempt), Some(2), false),
            AlignmentAction::Clear
        );
        // TTL 内旧场次配新目录（或相反）首次不对齐：读取都已返回时发起一次失效后的重读。
        assert_eq!(
            plan_alignment_reread(false, "a:live", None, Some(1), false),
            AlignmentAction::Reread
        );
        // 仍有读取在途（例如轮询时目录先返回、场次还在读）：返回后可能就对齐，先不重读。
        assert_eq!(
            plan_alignment_reread(false, "a:live", None, Some(1), true),
            AlignmentAction::Wait
        );
        // 同一上下文已重读过：不再自动重读（有界），用尽后只由用户重试清掉记录再发起。
        for (loaded, reading) in
            [(Some(1), false), (Some(2), false), (Some(2), true), (None, false)]
        {
            assert_eq!(
                plan_alignment_reread(false, "a:live", Some(&attempt), loaded, reading),
                AlignmentAction::Wait
            );
        }
    }

    #[test]
    fn a_new_context_waits_until_the_previous_round_has_returned() {
        // 上一轮为预览上下文发起（代号 4）；场次重读返回后上下文已切到开场后的 live（切换世界或
        // 场次时同理：旧上下文的尝试不算新上下文的重读，也不让新上下文判定用尽）。
        let attempt = ("a:preview:1".to_owned(), 4);
        // 目录仍是上一轮之前的快照，或仍有读取在途：先等待，不取消在途读取、不反复刷新。
        assert_eq!(
            plan_alignment_reread(false, "a:live:1", Some(&attempt), Some(3), false),
            AlignmentAction::Wait
        );
        assert_eq!(
            plan_alignment_reread(false, "a:live:1", Some(&attempt), Some(4), true),
            AlignmentAction::Wait
        );
        // 上一轮已全部返回仍不对齐：为新上下文发起它自己的一次重读。
        assert_eq!(
            plan_alignment_reread(false, "a:live:1", Some(&attempt), Some(4), false),
            AlignmentAction::Reread
        );
    }

    /// 组件调度替身的两侧读取：当前（或历史）场次与目录。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Side {
        Auction,
        Lots,
    }

    /// 一次读取的放行槽：测试写入结果并唤醒任务；读取结束或随资源重启被取消时作废。
    #[derive(Default)]
    struct Slot {
        reply: RefCell<Option<&'static str>>,
        waker: RefCell<Option<Waker>>,
        dropped: Cell<bool>,
    }

    /// 一次目录渲染看到的调度状态。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Frame {
        aligned: bool,
        reading: bool,
        exhausted: bool,
    }

    #[derive(Default)]
    struct Script {
        reads: Vec<(Side, Rc<Slot>)>,
        frames: Vec<Frame>,
        refresh: Option<Signal<u64>>,
    }

    thread_local! {
        static SCRIPT: RefCell<Script> = RefCell::new(Script::default());
    }

    /// 由测试放行的读取 future；被资源重启丢弃即作废，之后不再接受放行。
    struct Reply(Rc<Slot>);

    impl Future for Reply {
        type Output = &'static str;

        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<&'static str> {
            match self.0.reply.borrow_mut().take() {
                Some(reply) => Poll::Ready(reply),
                None => {
                    *self.0.waker.borrow_mut() = Some(context.waker().clone());
                    Poll::Pending
                }
            }
        }
    }

    impl Drop for Reply {
        fn drop(&mut self) {
            self.0.dropped.set(true);
        }
    }

    /// 发起一次读取并登记；资源每次启动或重启都在这里留下记录。
    fn read(side: Side) -> Reply {
        let slot = Rc::new(Slot::default());
        SCRIPT.with(|script| script.borrow_mut().reads.push((side, Rc::clone(&slot))));
        Reply(slot)
    }

    /// 把调度跑到没有新工作：轮询被唤醒的任务、执行 effect 并重渲染。读取都由测试放行，不会自行推进。
    fn settle(dom: &mut VirtualDom) {
        for _ in 0..32 {
            dom.process_events();
            dom.render_immediate(&mut NoOpMutations);
        }
    }

    /// 放行该侧仍在途的那次读取，并让调度跑完。
    fn reply(dom: &mut VirtualDom, side: Side, value: &'static str) {
        let slot = SCRIPT.with(|script| {
            script
                .borrow()
                .reads
                .iter()
                .rev()
                .find(|(read_side, slot)| *read_side == side && !slot.dropped.get())
                .map(|(_, slot)| Rc::clone(slot))
        });
        let slot = slot.unwrap_or_else(|| panic!("{side:?} 没有在途读取"));
        *slot.reply.borrow_mut() = Some(value);
        if let Some(waker) = slot.waker.borrow_mut().take() {
            waker.wake();
        }
        settle(dom);
    }

    /// 外壳轮询、恢复可见等统一刷新：失效公开读缓存并推进全局刷新代号，两侧一起重读。
    fn refresh(dom: &mut VirtualDom) {
        let signal = SCRIPT.with(|script| script.borrow().refresh);
        let signal = signal.unwrap_or_else(|| panic!("看板尚未挂载"));
        dom.in_runtime(|| refresh_public_reads(signal));
        settle(dom);
    }

    /// 已发起的读取次数（含随资源重启被取消的），依次为场次与目录。
    fn started() -> (usize, usize) {
        SCRIPT.with(|script| {
            let reads = &script.borrow().reads;
            let count = |side| reads.iter().filter(|(read_side, _)| *read_side == side).count();
            (count(Side::Auction), count(Side::Lots))
        })
    }

    fn frames() -> Vec<Frame> {
        SCRIPT.with(|script| script.borrow().frames.clone())
    }

    fn last_frame() -> Frame {
        frames().last().copied().unwrap_or_else(|| panic!("目录尚未渲染"))
    }

    /// 与 `AuctionBoard` 同形的父组件：持有全局刷新代号与场次资源，把读取在途状态交给目录。
    #[component]
    fn Board() -> Element {
        let refresh_generation = use_context_provider(|| Signal::new(0_u64));
        use_hook(|| SCRIPT.with(|script| script.borrow_mut().refresh = Some(refresh_generation)));
        let auction = use_resource(move || {
            let _ = refresh_generation();
            read(Side::Auction)
        });
        let auction_reading = *auction.state().read() == UseResourceState::Pending;
        let shown = *auction.read();
        match shown {
            None => rsx! {},
            Some(shown) => rsx! {
                Catalog { auction: shown.to_owned(), auction_reading }
            },
        }
    }

    /// 与 `AuctionData` 同口径的目录：资源带上产生它的目录刷新代号，没有快照时不判不对齐，
    /// 读取在途含父组件的场次读取；调度交给被测 hook。
    #[component]
    fn Catalog(auction: String, auction_reading: bool) -> Element {
        let refresh_generation = use_context::<Signal<u64>>();
        let lots_refresh_generation = use_signal(|| 0_u64);
        let lots = use_resource(move || {
            let _ = refresh_generation();
            let generation = lots_refresh_generation();
            let reply = read(Side::Lots);
            async move { (generation, reply.await) }
        });
        let loaded = *lots.read();
        let aligned = loaded.is_none_or(|(_, lots)| lots == auction);
        let loaded_generation = loaded.map(|(generation, _)| generation);
        let reading = auction_reading || *lots.state().read() == UseResourceState::Pending;
        let alignment = use_alignment_reread(
            aligned,
            auction,
            loaded_generation,
            reading,
            lots_refresh_generation,
            refresh_generation,
        );
        let frame = Frame { aligned, reading, exhausted: alignment.exhausted };
        SCRIPT.with(|script| script.borrow_mut().frames.push(frame));
        rsx! {}
    }

    /// 挂载看板，场次与目录先返回同一个已对齐的快照。
    fn mounted(initial: &'static str) -> VirtualDom {
        SCRIPT.with(|script| *script.borrow_mut() = Script::default());
        let mut dom = VirtualDom::new(Board);
        dom.rebuild_in_place();
        settle(&mut dom);
        reply(&mut dom, Side::Auction, initial);
        reply(&mut dom, Side::Lots, initial);
        assert_eq!(last_frame(), Frame { aligned: true, reading: false, exhausted: false });
        dom
    }

    /// 统一刷新后场次先返回新阶段、目录仍在途：先等待而不是重读（重读只会取消在途目录），目录返回
    /// 后自然对齐；没有额外读取，也不闪现错误面板。
    #[test]
    fn auction_returning_first_waits_for_the_inflight_catalog() {
        let mut dom = mounted("preview");
        refresh(&mut dom);
        reply(&mut dom, Side::Auction, "live");
        assert_eq!(last_frame(), Frame { aligned: false, reading: true, exhausted: false });

        reply(&mut dom, Side::Lots, "live");

        assert_eq!(last_frame(), Frame { aligned: true, reading: false, exhausted: false });
        assert_eq!(started(), (2, 2), "读取在途时不得发起对齐重读");
        assert!(frames().iter().all(|frame| !frame.exhausted), "不得闪现错误面板");
    }

    /// 目录先返回新阶段、场次仍在途：同样先等待，场次返回后对齐。
    #[test]
    fn catalog_returning_first_waits_for_the_inflight_auction() {
        let mut dom = mounted("preview");
        refresh(&mut dom);
        reply(&mut dom, Side::Lots, "live");
        assert_eq!(last_frame(), Frame { aligned: false, reading: true, exhausted: false });

        reply(&mut dom, Side::Auction, "live");

        assert_eq!(last_frame(), Frame { aligned: true, reading: false, exhausted: false });
        assert_eq!(started(), (2, 2), "读取在途时不得发起对齐重读");
        assert!(frames().iter().all(|frame| !frame.exhausted), "不得闪现错误面板");
    }

    /// 两侧都返回仍不对齐时发起一次失效后的重读；重读期间场次切到新上下文：先等上一轮目录返回，
    /// 再只为新上下文重读一次，最终对齐。读取次数有界：没有取消循环、错误面板闪现或永久等待。
    #[test]
    fn context_switch_during_the_reread_gets_one_reread_and_then_aligns() {
        let mut dom = mounted("preview");
        refresh(&mut dom);
        reply(&mut dom, Side::Auction, "live");
        reply(&mut dom, Side::Lots, "preview");
        assert_eq!(started(), (3, 3), "两侧都返回仍不对齐：对齐重读同时重读场次与目录");

        reply(&mut dom, Side::Auction, "final");
        assert_eq!(last_frame(), Frame { aligned: false, reading: true, exhausted: false });
        assert_eq!(started(), (3, 3), "上一轮目录仍在途：新上下文先等待");
        reply(&mut dom, Side::Lots, "live");
        assert_eq!(started(), (4, 4), "上一轮已返回仍不对齐：为新上下文重读一次");

        reply(&mut dom, Side::Auction, "final");
        reply(&mut dom, Side::Lots, "final");

        assert_eq!(last_frame(), Frame { aligned: true, reading: false, exhausted: false });
        assert_eq!(started(), (4, 4), "对齐后不再重读");
        assert!(frames().iter().all(|frame| !frame.exhausted), "不得闪现错误面板");
    }

    /// 同一上下文的那次重读返回后仍不对齐（例如读闸繁忙时回退的旧目录）：目录先返回、场次仍在途时
    /// 继续等待，两侧都返回后才判定用尽并显示可重试的错误；用尽后不再自动重读。
    #[test]
    fn exhaustion_is_reported_only_after_both_reread_sides_return() {
        let mut dom = mounted("preview");
        refresh(&mut dom);
        reply(&mut dom, Side::Auction, "live");
        reply(&mut dom, Side::Lots, "preview");
        assert_eq!(started(), (3, 3));

        reply(&mut dom, Side::Lots, "preview");
        assert_eq!(last_frame(), Frame { aligned: false, reading: true, exhausted: false });

        reply(&mut dom, Side::Auction, "live");

        assert_eq!(last_frame(), Frame { aligned: false, reading: false, exhausted: true });
        assert_eq!(started(), (3, 3), "用尽后不再自动重读");
    }
}
