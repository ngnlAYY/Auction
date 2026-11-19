//! Public 看板世界选择与世界标签辅助。

#[cfg(test)]
use std::rc::Rc;

use auction_time::Timestamp;
use auction_web_shared::validation::restore_select_value;
use auction_web_shared::{PageVisibility, QueryWriteMode, t};
use dioxus::prelude::*;

use crate::api::{WorldStatus, Worlds, revalidate_worlds};
use crate::surface::router::Route;
use crate::surface::shell::WorldCatalog;
use crate::surface::{
    AuctionParameter, AuctionSelection, PublicBoardQuery, QueryCommit, commit_auction_selection,
    commit_public_query_pairs, public_auction_home_query, read_public_query,
    update_public_query_state,
};

/// 与世界目录的低频 HTTP 缓存档对齐。
const WORLD_DIRECTORY_TTL_MILLIS: u32 = 300_000;

/// 公开端世界选择器；世界是否可用由服务端目录决定，切换后重新读取权威当前拍卖。
/// URL 写回经外壳共享的 query hook 状态信号（context），保证 hook 缓存与地址栏一致，
/// 浏览器前进/后退时 hook 的相等判断才能正确触发恢复。
#[component]
pub(crate) fn WorldPicker(mut selected_world: Signal<String>) -> Element {
    let route = use_route::<Route>();
    let history_owns_world_filter = matches!(
        &route,
        Route::AuctionList {} | Route::BidLog {} | Route::Fulfillment {} | Route::History {}
    );
    let board_route = matches!(&route, Route::AuctionIndex {} | Route::AuctionPage {});
    let shell_query_state = use_context::<Signal<PublicBoardQuery>>();
    let historical_url = history_owns_world_filter
        || has_auction_target(shell_query_state.read().auction_parameter());
    let worlds = use_context::<WorldCatalog>().0;
    let (mut options, worlds_loaded) = match &*worlds.read() {
        Some(worlds) => (header_worlds(worlds), true),
        _ => (vec!["main".to_owned(), "isekai".to_owned()], false),
    };
    let current = selected_world();
    let options_empty = options.is_empty();
    let current_disabled = !options.contains(&current);
    let fallback = if worlds_loaded && current_disabled && !historical_url {
        options.iter().find(|world| world.as_str() == "main").or_else(|| options.first()).cloned()
    } else {
        None
    };
    // 拒写后仍选中已提交的世界，保留可用选项供用户重试。历史深链也保留其真实世界。
    if current_disabled {
        options.push(current.clone());
    }
    use_effect(use_reactive((&fallback,), move |(fallback,)| {
        let Some(fallback) = fallback else { return };
        // 目录返回时地址可能已经换到指定场次或无效深链；只有仍无显式场次才可纠偏。
        if selected_world() == fallback
            || has_auction_target(read_public_query().auction_parameter())
        {
            return;
        }
        if replace_world_query(shell_query_state, (fallback != "main").then_some(&fallback)) {
            selected_world.set(fallback);
        }
    }));

    rsx! {
        label { class: "public-world-switcher",
            span { {t!("world-label")} }
            select {
                id: "public-world",
                name: "world",
                value: current,
                disabled: options_empty,
                onchange: move |event| {
                    let world = event.value();
                    if history_owns_world_filter {
                        selected_world.set(world);
                        return;
                    }
                    // 补偿池等页面也用 `auction` 参数，只有看板上的往期场次随世界切换离开。
                    let historical_board = board_route && matches!(
                        read_public_query().auction_parameter(), AuctionParameter::Selected(_)
                    );
                    // 用户主动切换要建立可返回的历史条目。
                    let next = if historical_board {
                        match commit_auction_selection(
                            shell_query_state,
                            AuctionSelection::Explicit(None),
                            None,
                            |query| *query = world_switch_query(query, &world, historical_board),
                        ) {
                            QueryCommit::Applied(applied) => Some(applied.world.as_deref().unwrap_or("main").to_owned()),
                            _ => None,
                        }
                    } else {
                        update_public_query_state(
                            shell_query_state,
                            |query| *query = world_switch_query(query, &world, historical_board),
                            QueryWriteMode::Push,
                        ).then_some(world)
                    };
                    if let Some(next) = next {
                        selected_world.set(next);
                    } else {
                        // 原生 change 已改动选项，拒写时须还原 DOM 才能重新选择同一个世界。
                        restore_select_value(&event, &selected_world.peek());
                    }
                },
                for world in options {
                    option {
                        key: "{world}",
                        value: world.clone(),
                        selected: current == world,
                        disabled: current_disabled && world == current,
                        {world_label(&world)}
                    }
                }
            }
        }
    }
}

/// 世界自动纠偏只拥有 `world`，其他片段随当前历史条目保留。
pub(super) fn replace_world_query(state: Signal<PublicBoardQuery>, world: Option<&str>) -> bool {
    let pairs =
        world.into_iter().map(|world| ("world".to_owned(), world.to_owned())).collect::<Vec<_>>();
    commit_public_query_pairs(state, &["world"], &pairs, QueryWriteMode::Replace)
}

fn has_auction_target(auction: &AuctionParameter) -> bool {
    *auction != AuctionParameter::Unspecified
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct WorldRead {
    checked_at: Timestamp,
    succeeded: bool,
}

impl WorldRead {
    fn needs_revalidation_on_resume(self, now: Timestamp) -> bool {
        !self.succeeded
            || now < self.checked_at
            || (now - self.checked_at).num_milliseconds() >= i64::from(WORLD_DIRECTORY_TTL_MILLIS)
    }
}

impl WorldCatalog {
    /// 外壳从启动起读取目录，并在到期或恢复可见时再验证。
    pub(in crate::surface) fn use_revalidation(
        self,
        visibility_generation: Signal<u64>,
        visibility: PageVisibility,
    ) {
        let mut snapshot = self.0;
        let worlds = use_resource(move || async move {
            let read = read_world_catalog().await;
            let succeeded = read.is_ok();
            // 失败时保留最近确认的原始目录。恢复只发布给当前订阅者，不捕获页面实例。
            if let Ok(worlds) = read
                && snapshot.peek().as_ref() != Some(&worlds)
            {
                snapshot.set(Some(worlds));
            }
            WorldRead { checked_at: world_catalog_now(), succeeded }
        });
        #[cfg(any(target_arch = "wasm32", test))]
        let timer_visibility = visibility.clone();
        use_effect(move || {
            if visibility_generation() != 0 {
                revalidate_on_resume(worlds, world_catalog_now(), visibility.is_visible());
            }
        });
        #[cfg(any(target_arch = "wasm32", test))]
        use_resource(move || {
            let read = *worlds.read();
            let visibility = timer_visibility.clone();
            async move {
                let Some(read) = read else { return };
                wait_for_world_revalidation().await;
                // 隐藏期间到期时保留这次唤醒，恢复可见不依赖墙钟再次判断过期。
                visibility.wait_until_visible().await;
                if *worlds.peek() == Some(read) {
                    // 定时器已走完一个周期，墙钟回拨不能取消本轮请求及后续计时。
                    restart_while_visible(worlds, visibility.is_visible());
                }
            }
        });
    }
}

fn world_catalog_now() -> Timestamp {
    #[cfg(test)]
    if let Some(fixture) = try_consume_context::<Rc<tests::CatalogTimerFixture>>() {
        return fixture.now.get();
    }
    Timestamp::now()
}

#[cfg(any(target_arch = "wasm32", test))]
async fn wait_for_world_revalidation() {
    #[cfg(test)]
    if let Some(fixture) = try_consume_context::<Rc<tests::CatalogTimerFixture>>() {
        fixture.wait().await;
        return;
    }
    auction_web_shared::timer::TimeoutFuture::new(WORLD_DIRECTORY_TTL_MILLIS).await;
}

fn revalidate_on_resume(worlds: Resource<WorldRead>, now: Timestamp, visible: bool) {
    if worlds.peek().is_some_and(|read| read.needs_revalidation_on_resume(now)) {
        restart_while_visible(worlds, visible);
    }
}

fn restart_while_visible(mut worlds: Resource<WorldRead>, visible: bool) {
    // 周期与可见性恢复共用一个资源；在途时不取消或叠加请求。
    if visible && !worlds.pending() {
        worlds.restart();
    }
}

async fn read_world_catalog() -> Result<Worlds, String> {
    #[cfg(test)]
    if let Some(fixture) = try_consume_context::<Rc<tests::CatalogTimerFixture>>() {
        return fixture.read().await;
    }
    revalidate_worlds().await
}

/// 页头切换世界后的查询。看板正在展示往期场次时回到目标世界的首页（清除场次、筛选与页码，
/// 每页条数取本地偏好），与顶栏首页入口一致，避免页头世界与正文、顶栏的场次不一致；
/// 其余情况只改世界。
fn world_switch_query(
    current: &PublicBoardQuery,
    world: &str,
    historical_board: bool,
) -> PublicBoardQuery {
    if historical_board {
        return public_auction_home_query(world);
    }
    PublicBoardQuery {
        // 主世界沿用旧版无参数 URL。
        world: (world != "main").then(|| world.to_owned()).into(),
        ..current.clone()
    }
}

fn header_worlds(worlds: &Worlds) -> Vec<String> {
    // 旧版顶部选择器直接沿用 worlds 对象的稳定键顺序，但只接受两个冻结世界。
    worlds
        .worlds
        .iter()
        .filter(|(world, status)| is_supported_world(world) && status.enabled)
        .map(|(world, _): (&String, &WorldStatus)| world.clone())
        .collect()
}

/// 历史筛选保留两个受支持世界；业务开关只隐藏当前/可操作世界，不抹除 settled 事实。
pub(crate) fn supported_worlds(worlds: &Worlds) -> Vec<String> {
    let mut supported = worlds
        .worlds
        .iter()
        .filter(|(world, _)| is_supported_world(world))
        .map(|(world, _): (&String, &WorldStatus)| world.clone())
        .collect::<Vec<_>>();
    // 旧版页面筛选器固定以 Main 为首项。
    supported.sort_by_key(|world| match world.as_str() {
        "main" => 0_u8,
        "isekai" => 1_u8,
        _ => 2_u8,
    });
    supported
}

fn is_supported_world(world: &str) -> bool {
    matches!(world, "main" | "isekai")
}

pub(crate) fn world_label(world: &str) -> String {
    match world {
        "main" => t!("auction-world-main"),
        "isekai" => t!("auction-world-isekai"),
        _ => world.to_owned(),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::poll_fn;
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    use auction_time::Timestamp;
    use auction_web_shared::PageVisibility;
    use dioxus::prelude::*;

    use super::{
        AuctionParameter, PublicBoardQuery, WorldRead, has_auction_target, header_worlds,
        restart_while_visible, revalidate_on_resume, world_switch_query,
    };
    use crate::api::{WorldStatus, Worlds};
    use crate::surface::reads::tests::select_values;
    use crate::surface::reads::{bid_log, fulfillment};
    use crate::surface::shell::WorldCatalog;
    use crate::surface::{PageBrowserFixture, PageReadFixture, page_test_contexts, settle_page};

    pub(in crate::surface::board) struct CatalogTimerFixture {
        pub(super) now: Cell<Timestamp>,
        enabled_world: &'static str,
        next_read: Cell<Option<Result<Worlds, String>>>,
        pause_read: Cell<bool>,
        reading: Cell<Option<Waker>>,
        calls: Cell<usize>,
        elapsed: Cell<u64>,
        timer: RefCell<Option<Waker>>,
    }

    impl CatalogTimerFixture {
        pub(in crate::surface::board) fn new(enabled_world: &'static str, now: Timestamp) -> Self {
            Self {
                now: Cell::new(now),
                enabled_world,
                next_read: Cell::new(None),
                pause_read: Cell::new(false),
                reading: Cell::new(None),
                calls: Cell::new(0),
                elapsed: Cell::new(0),
                timer: RefCell::default(),
            }
        }

        pub(super) async fn read(&self) -> Result<Worlds, String> {
            self.calls.update(|n| n + 1);
            poll_fn(|context| {
                if self.pause_read.get() {
                    self.reading.set(Some(context.waker().clone()));
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            self.next_read.take().unwrap_or_else(|| {
                Ok(Worlds {
                    worlds: [(self.enabled_world.to_owned(), WorldStatus { enabled: true })].into(),
                })
            })
        }

        pub(super) async fn wait(&self) {
            let since = self.elapsed.get();
            poll_fn(|context| {
                if self.elapsed.get() > since {
                    Poll::Ready(())
                } else {
                    self.timer.replace(Some(context.waker().clone()));
                    Poll::Pending
                }
            })
            .await;
        }

        fn fire_at(&self, now: Timestamp) {
            self.now.set(now);
            self.elapsed.update(|n| n + 1);
            self.timer.take().expect("目录已安排下一次再验证").wake();
        }
    }

    #[component]
    fn CatalogTimerHarness() -> Element {
        let visible_generation = use_signal(|| 0);
        let visibility = use_hook(PageVisibility::new);
        use_context_provider(WorldCatalog::new).use_revalidation(visible_generation, visibility);
        rsx! {}
    }

    #[test]
    fn elapsed_catalog_timer_revalidates_even_if_wall_clock_moved_back() {
        let timestamp = |seconds: i64| Timestamp::from_timestamp_millis(seconds * 1_000).unwrap();
        let fixture = Rc::new(CatalogTimerFixture::new("main", timestamp(0)));
        let mut dom = VirtualDom::new(CatalogTimerHarness).with_root_context(fixture.clone());
        dom.rebuild_in_place();
        let settle = |dom: &mut VirtualDom| {
            for _ in 0..4 {
                dom.process_events();
                dom.render_immediate_to_vec();
            }
        };
        settle(&mut dom);
        assert_eq!(fixture.calls.get(), 1);
        fixture.fire_at(timestamp(295));
        settle(&mut dom);
        assert_eq!(fixture.calls.get(), 2);
        fixture.fire_at(timestamp(595));
        settle(&mut dom);
        assert_eq!(fixture.calls.get(), 3, "下一轮计时仍在运行");
    }

    type ActiveReadPage = Rc<Cell<Option<Signal<bool>>>>;

    #[component]
    fn ReadPageCatalogHarness() -> Element {
        page_test_contexts();
        let visible_generation = use_signal(|| 0);
        let visibility = use_hook(PageVisibility::new);
        use_context_provider(WorldCatalog::new).use_revalidation(visible_generation, visibility);
        let show_fulfillment = use_signal(|| false);
        consume_context::<ActiveReadPage>().set(Some(show_fulfillment));
        rsx! {
            if show_fulfillment() {
                CatalogFulfillment {}
            } else {
                CatalogBidLog {}
            }
        }
    }

    #[component]
    fn CatalogFulfillment() -> Element {
        fulfillment()
    }

    #[component]
    fn CatalogBidLog() -> Element {
        bid_log()
    }

    #[test]
    fn world_catalog_recovers_the_currently_mounted_read_page() {
        let timestamp = |seconds: i64| Timestamp::from_timestamp_millis(seconds * 1_000).unwrap();
        let fixture = Rc::new(CatalogTimerFixture::new("isekai", timestamp(0)));
        fixture.next_read.set(Some(Err("unavailable".to_owned())));
        let page = ActiveReadPage::default();
        let page_reads = Rc::new(Cell::new(0));
        let mut dom = VirtualDom::new(ReadPageCatalogHarness)
            .with_root_context(fixture.clone())
            .with_root_context(page.clone())
            .with_root_context(PageBrowserFixture::new("/fulfillment", "world=invalid"))
            .with_root_context(PageReadFixture::new({
                let page_reads = page_reads.clone();
                move |()| {
                    page_reads.update(|count| count + 1);
                    Err::<Worlds, _>("unavailable".to_owned())
                }
            }));
        dom.rebuild_in_place();
        settle_page(&mut dom);
        assert_eq!(fixture.calls.get(), 1);

        let disabled_worlds =
            Worlds { worlds: [("isekai".to_owned(), WorldStatus { enabled: false })].into() };
        assert!(header_worlds(&disabled_worlds).is_empty());
        fixture.next_read.set(Some(Ok(disabled_worlds)));
        fixture.pause_read.set(true);
        fixture.fire_at(timestamp(300));
        settle_page(&mut dom);
        assert_eq!(fixture.calls.get(), 2);
        let in_flight = fixture.reading.take().expect("旧页面挂载时已开始再验证");
        dom.in_runtime(|| page.get().unwrap().set(true));
        settle_page(&mut dom);
        assert_eq!(fixture.calls.get(), 2, "换页继续复用外壳的在途读取");
        let options = |dom: &VirtualDom| select_values(dom, "id", "public-fulfillment-world");
        assert_eq!(options(&dom), ["", "main", "isekai"]);

        fixture.pause_read.set(false);
        in_flight.wake();
        settle_page(&mut dom);
        assert_eq!(fixture.calls.get(), 2);
        assert_eq!(options(&dom), ["", "isekai"]);

        fixture.next_read.set(Some(Err("unavailable".to_owned())));
        fixture.fire_at(timestamp(600));
        settle_page(&mut dom);
        assert_eq!(fixture.calls.get(), 3);
        assert_eq!(options(&dom), ["", "isekai"]);
        assert_eq!(page_reads.get(), 0, "切页和恢复都无需页面发起第二份目录读取");
    }

    #[derive(Default)]
    struct CatalogFixture {
        calls: Cell<usize>,
        checked: Cell<Option<WorldRead>>,
        resource: Cell<Option<Resource<WorldRead>>>,
    }

    #[component]
    fn CatalogReads() -> Element {
        let fixture = consume_context::<Rc<CatalogFixture>>();
        let reads = fixture.clone();
        let resource = use_resource(move || {
            reads.calls.update(|n| n + 1);
            std::future::ready(reads.checked.get().unwrap())
        });
        fixture.resource.set(Some(resource));
        rsx! {}
    }

    #[test]
    fn world_catalog_checks_expired_or_failed_reads_when_visible_without_duplicate_requests() {
        let timestamp = |seconds: i64| Timestamp::from_timestamp_millis(seconds * 1_000).unwrap();
        let fixture = Rc::new(CatalogFixture::default());
        fixture.checked.set(Some(WorldRead { checked_at: timestamp(0), succeeded: true }));
        let mut dom = VirtualDom::new(CatalogReads).with_root_context(fixture.clone());
        dom.rebuild_in_place();
        dom.process_events();
        let resource = fixture.resource.get().unwrap();

        dom.in_runtime(|| revalidate_on_resume(resource, timestamp(299), true));
        dom.in_runtime(|| restart_while_visible(resource, false));
        assert_eq!(fixture.calls.get(), 1);

        fixture.checked.set(Some(WorldRead { checked_at: timestamp(300), succeeded: false }));
        dom.in_runtime(|| {
            restart_while_visible(resource, true);
            revalidate_on_resume(resource, timestamp(300), true);
        });
        dom.process_events();
        assert_eq!(fixture.calls.get(), 2, "周期和恢复可见只取一次");

        fixture.checked.set(Some(WorldRead { checked_at: timestamp(301), succeeded: true }));
        dom.in_runtime(|| revalidate_on_resume(resource, timestamp(301), false));
        assert_eq!(fixture.calls.get(), 2, "隐藏期间不重试失败");
        dom.in_runtime(|| revalidate_on_resume(resource, timestamp(301), true));
        dom.process_events();
        assert_eq!(fixture.calls.get(), 3, "恢复可见补读失败目录");
        dom.in_runtime(|| revalidate_on_resume(resource, timestamp(302), true));
        assert_eq!(fixture.calls.get(), 3);
    }

    fn historical_query() -> PublicBoardQuery {
        PublicBoardQuery {
            category: Some("clo".to_owned()).into(),
            equipment_group: Some("rare".to_owned()),
            max_level: Some("400".to_owned()),
            page: 3,
            page_size: 50,
            ..PublicBoardQuery::default()
        }
        .with_auction_values(["67e55044-10b1-426f-9247-bb680e5fe0c8".to_owned()])
    }

    #[test]
    fn switching_world_on_a_historical_board_returns_to_the_world_home() {
        let query = world_switch_query(&historical_query(), "isekai", true);
        // 场次、筛选与页码全部清除，每页条数取本地偏好。
        assert_eq!(
            query,
            PublicBoardQuery {
                world: Some("isekai".to_owned()).into(),
                ..PublicBoardQuery::default()
            }
        );
        assert_eq!(
            world_switch_query(&historical_query(), "main", true),
            PublicBoardQuery::default()
        );
    }

    #[test]
    fn switching_world_elsewhere_only_changes_the_world() {
        let current = historical_query();
        let query = world_switch_query(&current, "isekai", false);
        assert_eq!(query, PublicBoardQuery { world: Some("isekai".to_owned()).into(), ..current });
        let current =
            PublicBoardQuery { world: Some("isekai".to_owned()).into(), ..historical_query() };
        assert_eq!(world_switch_query(&current, "main", false).world.as_deref(), None);
    }

    #[test]
    fn world_correction_preserves_invalid_and_selected_board_links() {
        let selected = historical_query().auction_id().unwrap().to_owned();
        assert!(has_auction_target(&AuctionParameter::Selected(selected)));
        assert!(has_auction_target(&AuctionParameter::Invalid(vec!["bad".to_owned()])));
        assert!(!has_auction_target(&AuctionParameter::Unspecified));
    }
}
