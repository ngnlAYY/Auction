//! Public 语言目录与启动入口；路由表见子模块 `route`，页面外壳见 `shell` 模块。

#[cfg(any(target_arch = "wasm32", test))]
mod fragment;
#[cfg(any(target_arch = "wasm32", test))]
mod history;

// dioxus-router-macro 0.7.10 的 `Routable` 派生把 `String`/`Vec<String>` 段的解析错误包进
// `map_err` 闭包；Rust 1.100 起 `Infallible` 即 `!`，该闭包被判为不可达调用。派生代码在 `const _`
// 块中与 `Route` 并列，枚举上的 lint 属性覆盖不到，因此路由表单独成子模块，只对该子模块放行，
// 本模块其余代码照常受 lint 约束；上游修正派生后删除。
#[allow(unreachable_code, reason = "Routable 派生的段解析错误映射在 Infallible 即 ! 后不可达")]
mod route;

pub(crate) use route::Route;

#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use auction_web_shared::i18n::{
    DEFAULT_PUBLIC_LOCALE, LocaleCatalog, LocaleSession, LocaleSource, build_bundle,
};
#[cfg(target_arch = "wasm32")]
use auction_web_shared::query::QueryLocation;
use auction_web_shared::{
    BaseStylesheet, StaleBuildBanner, t, use_locale_effects, use_theme_effects,
};
#[cfg(target_arch = "wasm32")]
use dioxus::history::provide_history_context;
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
use unic_langid::langid;

#[cfg(target_arch = "wasm32")]
use self::fragment::FragmentScroll;
#[cfg(target_arch = "wasm32")]
use self::history::{PublicHistory, write_browser_location};
use super::login_return::{
    initial_login_return_location, use_expire_initial_login_return_location,
};

/// 公开端 surface 样式。设计变量由 `auction-web-shared` 提供。
const PUBLIC_CSS: Asset = asset!("/assets/public.css", AssetOptions::css().with_static_head(true));
const DESKTOP_CSS: Asset =
    asset!("/assets/desktop.css", AssetOptions::css().with_static_head(true));
const MOBILE_CSS: Asset = asset!("/assets/mobile.css", AssetOptions::css().with_static_head(true));

/// Public 语言目录：默认英文编译期内嵌，其余四种从带指纹的静态资源按需取回。
pub(crate) static PUBLIC_LOCALE_CATALOG: LocaleCatalog = LocaleCatalog {
    default_locale: DEFAULT_PUBLIC_LOCALE,
    default_ftl: include_str!("../../../i18n/en.ftl"),
    sources: &[
        LocaleSource { id: langid!("zh-CN"), asset: asset!("/i18n/zh-CN.ftl") },
        LocaleSource { id: langid!("zh-TW"), asset: asset!("/i18n/zh-TW.ftl") },
        LocaleSource { id: langid!("ja"), asset: asset!("/i18n/ja.ftl") },
        LocaleSource { id: langid!("ko"), asset: asset!("/i18n/ko.ftl") },
    ],
};

/// 页内一次性操作绑定历史条目，replace 纠偏不会改变条目身份。
#[derive(Clone, Default)]
pub(crate) struct PublicHistoryEntry(Arc<AtomicU64>);

impl PublicHistoryEntry {
    pub(crate) fn revision(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    #[cfg(any(target_arch = "wasm32", test))]
    fn advance(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn run() {
    dioxus::launch(App);
}

#[component]
pub(crate) fn App() -> Element {
    use_context_provider(initial_login_return_location);
    // 默认语言编译期内嵌；其余语言按目录从静态资源取回，已保存的非默认语言在首屏等待加载。
    let i18n_bundle = try_use_init_i18n_from_bundle(|| {
        build_bundle(
            &DEFAULT_PUBLIC_LOCALE,
            PUBLIC_LOCALE_CATALOG.default_ftl,
            &DEFAULT_PUBLIC_LOCALE,
            None,
        )
    });
    let Ok(i18n_bundle) = i18n_bundle else {
        return rsx! {
            div { class: "app-root", "Unable to initialize translations." }
        };
    };
    let mut session = use_hook(|| LocaleSession::new(i18n_bundle, DEFAULT_PUBLIC_LOCALE));
    use_hook(|| session.bootstrap(&PUBLIC_LOCALE_CATALOG));
    use_context_provider(|| session);
    use_locale_effects(session);
    // 首次渲染前恢复 hv.theme，应用显示时已是目标主题。
    use_theme_effects();
    if !session.is_ready() {
        return rsx! {
            div { class: "app-root" }
        };
    }

    // 样式只由静态 head 声明；这里的引用让三份 surface 样式进入构建产物。
    let _ = (PUBLIC_CSS, DESKTOP_CSS, MOBILE_CSS);
    rsx! {
        BaseStylesheet {}
        div { class: "app-root",
            StaleBuildBanner {
                message: t!("stale-build-notice"),
                refresh_label: t!("stale-build-refresh"),
            }
            PublicRouter {}
        }
    }
}

#[component]
fn PublicRouter() -> Element {
    #[cfg(target_arch = "wasm32")]
    use_public_history();
    use_expire_initial_login_return_location();
    rsx! { Router::<Route> {} }
}

#[cfg(target_arch = "wasm32")]
fn use_public_history() {
    let (history, scroll) = use_hook(|| {
        let browser = history();
        let prefix = browser.current_prefix().unwrap_or_default();
        let scroll = FragmentScroll::new(Rc::clone(&browser));
        let on_navigation = scroll.clone();
        let history =
            Rc::new(PublicHistory::<Route>::new(browser, move |location, mode, keep_scroll| {
                if !write_browser_location(&format!("{prefix}{location}"), mode, keep_scroll) {
                    return false;
                }
                if let Some(scroll) = &on_navigation {
                    scroll.enter(location, false);
                }
                true
            }));
        (history, scroll)
    });
    use_hook({
        let history = Rc::clone(&history);
        move || {
            provide_history_context(history.clone() as Rc<dyn History>);
            provide_context(history.entry());
            provide_context(history as Rc<dyn QueryLocation>);
        }
    });
    use_effect(move || {
        if let Some(scroll) = &scroll {
            scroll.listen();
            scroll.enter(&history.current_route(), false);
        }
    });
}

#[cfg(test)]
pub(crate) fn provide_test_public_history<R: Routable>(
    dom: &mut VirtualDom,
    memory: std::rc::Rc<dioxus::history::MemoryHistory>,
    allow: impl Fn(&str, auction_web_shared::QueryWriteMode) -> bool + 'static,
) {
    use std::rc::Rc;

    use auction_web_shared::QueryWriteMode;
    use auction_web_shared::query::QueryLocation;
    use dioxus::history::History;

    let public = Rc::new(history::PublicHistory::<R>::new(memory.clone(), move |url, mode, _| {
        if !allow(url, mode) {
            return false;
        }
        match mode {
            QueryWriteMode::Push => memory.push(url.to_owned()),
            QueryWriteMode::Replace => memory.replace(url.to_owned()),
        }
        true
    }));
    dom.provide_root_context(public.entry());
    dom.provide_root_context(public.clone() as Rc<dyn History>);
    dom.provide_root_context(public as Rc<dyn QueryLocation>);
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use auction_web_shared::query::QueryLocation;
    use auction_web_shared::{QueryWriteMode, read_query_pairs, write_query_pairs};
    use dioxus::dioxus_core::NoOpMutations;
    use dioxus::history::MemoryHistory;
    use dioxus::prelude::*;

    use super::history::PublicHistory;
    use crate::surface::{
        AuctionParameter, AuctionSelection, PublicBoardQuery, QueryCommit,
        commit_auction_selection, update_public_query_state, use_public_query_state,
    };

    #[derive(Clone, PartialEq, Routable)]
    enum TestRoute {
        #[route("/")]
        Index {},
        #[route("/me")]
        Me {},
        #[route("/rules")]
        Rules {},
        #[route("/history")]
        HistoryPage {},
        #[route("/bidlog")]
        BidLogFixture {},
    }

    #[component]
    fn Index() -> Element {
        rsx! { QueryPanel {} }
    }

    #[component]
    fn Me() -> Element {
        rsx! {}
    }

    #[component]
    fn Rules() -> Element {
        rsx! {}
    }

    #[component]
    fn HistoryPage() -> Element {
        rsx! { QueryPanel {} }
    }

    #[component]
    fn BidLogFixture() -> Element {
        rsx! { QueryPanel {} }
    }

    #[derive(Clone, Copy)]
    struct Actions {
        apply: Callback<(Option<&'static str>, i64), bool>,
        choose_auction: Callback<AuctionSelection, QueryCommit>,
        shared_size: Callback<i64, bool>,
        correct_world: Callback<&'static str, bool>,
        back: Callback<()>,
        push: Callback<&'static str>,
    }

    #[derive(Default)]
    struct View {
        actions: Cell<Option<Actions>>,
        address: RefCell<String>,
        query: RefCell<PublicBoardQuery>,
        header_world: RefCell<Option<String>>,
        pairs: RefCell<Vec<(String, String)>>,
    }

    #[component]
    fn QueryPanel() -> Element {
        let state = use_public_query_state().state;
        let header = use_public_query_state().state;
        let view = use_context::<Rc<View>>();
        let actions = Actions {
            apply: use_callback(move |(name, page): (Option<&str>, i64)| {
                update_public_query_state(
                    state,
                    |query| {
                        query.history_name = name.map(str::to_owned);
                        query.page = page;
                    },
                    QueryWriteMode::Push,
                )
            }),
            choose_auction: use_callback(move |selection| {
                let result = commit_auction_selection(state, selection, None, |query| {
                    query.page = 1;
                    query.bid_log_lot = None;
                    // 回调即使替换整份草稿，最终场次仍由提交的选择意图决定。
                    *query = query
                        .clone()
                        .with_auction_values(["0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b".to_owned()]);
                });
                if let QueryCommit::Applied(applied) = &result {
                    assert_eq!(*state.peek(), *applied);
                }
                result
            }),
            shared_size: use_callback(move |size: i64| {
                write_query_pairs(
                    &["page_size"],
                    &[("page_size".to_owned(), size.to_string())],
                    QueryWriteMode::Replace,
                )
            }),
            correct_world: use_callback(move |world: &str| {
                let auction = state.peek().auction_id().map(str::to_owned);
                matches!(
                    commit_auction_selection(
                        state,
                        AuctionSelection::Automatic(auction),
                        None,
                        |query| query.world = crate::surface::normalize_world_query(world).into(),
                    ),
                    QueryCommit::Applied(_)
                )
            }),
            back: use_callback(move |()| navigator().go_back()),
            push: use_callback(move |location: &str| {
                navigator().push(location);
            }),
        };
        view.actions.set(Some(actions));
        view.address.replace(router().full_route_string());
        view.query.replace(state());
        view.header_world.replace(header.read().world.as_deref().map(str::to_owned));
        view.pairs.replace(read_query_pairs());
        rsx! {}
    }

    fn fixture() -> Element {
        rsx! { Router::<TestRoute> {} }
    }

    struct Mounted {
        dom: VirtualDom,
        history: Rc<MemoryHistory>,
        view: Rc<View>,
        pushes: Rc<Cell<usize>>,
        rejects_writes: Rc<Cell<bool>>,
    }

    impl Mounted {
        fn new(address: &str) -> Self {
            let history = Rc::new(MemoryHistory::with_initial_path(address));
            let view = Rc::new(View::default());
            let pushes = Rc::new(Cell::new(0));
            let rejects_writes = Rc::new(Cell::new(false));
            let public = Rc::new(PublicHistory::<TestRoute>::new(history.clone(), {
                let history = history.clone();
                let pushes = pushes.clone();
                let rejects_writes = rejects_writes.clone();
                move |address, mode, _| {
                    if rejects_writes.get() {
                        return false;
                    }
                    match mode {
                        QueryWriteMode::Push => {
                            history.push(address.to_owned());
                            pushes.set(pushes.get() + 1);
                        }
                        QueryWriteMode::Replace => history.replace(address.to_owned()),
                    }
                    true
                }
            }));
            let mut dom = VirtualDom::new(fixture);
            dom.provide_root_context(public.clone() as Rc<dyn History>);
            dom.provide_root_context(public as Rc<dyn QueryLocation>);
            dom.provide_root_context(view.clone());
            dom.rebuild_in_place();
            let mut mounted = Self { dom, history, view, pushes, rejects_writes };
            mounted.settle();
            mounted
        }

        fn settle(&mut self) {
            for _ in 0..4 {
                self.dom.process_events();
                self.dom.render_immediate(&mut NoOpMutations);
            }
        }

        fn actions(&self) -> Actions {
            self.view.actions.get().unwrap()
        }
    }

    #[test]
    fn cold_start_keeps_query_and_fragment_when_canonicalizing_a_path() {
        for (address, expected) in [
            ("/me/?ref=a%26b#login-duration", "/me?ref=a%26b#login-duration"),
            ("/me#login-duration", "/me#login-duration"),
            ("/me/?#login-duration", "/me#login-duration"),
            ("/??page=2", "/??page=2"),
        ] {
            let mounted = Mounted::new(address);
            assert_eq!(mounted.history.current_route(), expected);
            assert_eq!(mounted.pushes.get(), 0);
        }
    }

    #[test]
    fn internal_navigation_normalizes_slashes_without_losing_query_or_fragment() {
        let mut mounted = Mounted::new("/");
        mounted.dom.in_runtime(|| mounted.actions().push.call("/rules/?ref=x#schedule"));
        mounted.settle();
        assert_eq!(mounted.history.current_route(), "/rules?ref=x#schedule");
        assert_eq!(mounted.pushes.get(), 1);
    }

    #[test]
    fn shared_page_size_write_preserves_the_literal_query_key() {
        let mut mounted = Mounted::new("/??page=2&ref=x");
        assert_eq!(mounted.view.query.borrow().page, 1);
        assert!(mounted.dom.in_runtime(|| mounted.actions().shared_size.call(50)));
        mounted.settle();
        assert_eq!(mounted.history.current_route(), "/??page=2&ref=x&page_size=50");
        assert_eq!(*mounted.view.address.borrow(), mounted.history.current_route());
        let query = mounted.view.query.borrow();
        assert_eq!((query.page, query.page_size), (1, 50));
        assert_eq!(
            query.passthrough,
            [("?page".to_owned(), "2".to_owned()), ("ref".to_owned(), "x".to_owned())]
        );
        assert_eq!(mounted.view.pairs.borrow()[0], ("?page".to_owned(), "2".to_owned()));

        let mut mounted = Mounted::new("/?page=2&ref=x");
        assert!(mounted.dom.in_runtime(|| mounted.actions().shared_size.call(50)));
        mounted.settle();
        assert_eq!(mounted.view.query.borrow().page, 2);
    }

    #[test]
    fn correcting_a_deep_links_world_reaches_the_independent_header_query() {
        const ID: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";
        let address = format!("/?auction={ID}");
        let mut mounted = Mounted::new(&address);
        assert!(mounted.view.header_world.borrow().is_none());
        mounted.rejects_writes.set(true);
        assert!(!mounted.dom.in_runtime(|| mounted.actions().correct_world.call("isekai")));
        mounted.settle();
        assert!(mounted.view.header_world.borrow().is_none());
        assert_eq!(mounted.history.current_route(), address);
        mounted.rejects_writes.set(false);
        assert!(mounted.dom.in_runtime(|| mounted.actions().correct_world.call("isekai")));
        mounted.settle();
        assert_eq!(mounted.view.query.borrow().auction_id(), Some(ID));
        assert_eq!(mounted.view.query.borrow().world.as_deref(), Some("isekai"));
        assert_eq!(mounted.view.header_world.borrow().as_deref(), Some("isekai"));
        assert!(mounted.history.current_route().contains("world=isekai"));
        assert_eq!(mounted.pushes.get(), 0);
    }

    #[test]
    fn selecting_or_clearing_an_invalid_auction_commits_a_complete_address() {
        const AUCTION: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";
        let original = "/bidlog?auction=&auction=bad&ref=x&bidlog_page=4&lot=One12";
        let mut mounted = Mounted::new(original);
        for new_target in [Some(AUCTION), None] {
            assert_matches!(
                mounted.view.query.borrow().auction_parameter(),
                AuctionParameter::Invalid(_)
            );
            let result = mounted.dom.in_runtime(|| {
                mounted
                    .actions()
                    .choose_auction
                    .call(AuctionSelection::Explicit(new_target.map(str::to_owned)))
            });
            let QueryCommit::Applied(applied) = result else { panic!("地址应已提交") };
            // 旧别名由序列化移除。候选还带有迁移标记，提交结果须取自新地址。
            assert!(!applied.bid_log_page_alias);
            assert_eq!(applied.page, 1);
            assert_eq!(applied.bid_log_lot, None);
            mounted.settle();
            let query = mounted.view.query.borrow();
            assert_eq!(*query, applied);
            let expected = new_target.map_or(AuctionParameter::Unspecified, |id| {
                AuctionParameter::Selected(id.to_owned())
            });
            assert_eq!(query.auction_parameter(), &expected);
            let pairs = mounted.view.pairs.borrow();
            assert_eq!(
                pairs.iter().find(|(key, _)| key == "auction").map(|(_, value)| value.as_str()),
                new_target
            );
            assert_eq!(
                pairs.iter().filter(|(key, _)| key == "auction").count(),
                usize::from(new_target.is_some())
            );
            drop(pairs);
            drop(query);
            mounted.dom.in_runtime(|| mounted.actions().back.call(()));
            mounted.settle();
            assert_eq!(mounted.history.current_route(), original);
        }
    }

    #[test]
    fn automatic_selection_reports_the_invalid_address_and_refused_writes_keep_it() {
        const AUCTION: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";
        let original = "/bidlog?auction=bad&page=4";
        let mut mounted = Mounted::new(original);
        let initial = mounted.view.query.borrow().clone();
        assert_eq!(
            mounted.dom.in_runtime(|| {
                mounted
                    .actions()
                    .choose_auction
                    .call(AuctionSelection::Automatic(Some(AUCTION.to_owned())))
            }),
            QueryCommit::InvalidAuction(initial.clone())
        );
        assert_eq!(
            mounted.dom.in_runtime(|| {
                mounted
                    .actions()
                    .choose_auction
                    .call(AuctionSelection::Explicit(Some("bad".to_owned())))
            }),
            QueryCommit::InvalidSelection
        );
        mounted.rejects_writes.set(true);
        assert_eq!(
            mounted.dom.in_runtime(|| {
                mounted.actions().choose_auction.call(AuctionSelection::Explicit(None))
            }),
            QueryCommit::Rejected
        );
        mounted.settle();
        assert_eq!(*mounted.view.query.borrow(), initial);
        assert_eq!(mounted.history.current_route(), original);
        assert_eq!(mounted.pushes.get(), 0);
    }

    #[test]
    fn automatic_selection_sets_a_valid_target_without_pushing_history() {
        const AUCTION: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";
        let mut mounted = Mounted::new("/bidlog?page=4");
        let result = mounted.dom.in_runtime(|| {
            mounted
                .actions()
                .choose_auction
                .call(AuctionSelection::Automatic(Some(AUCTION.to_owned())))
        });
        let QueryCommit::Applied(query) = result else { panic!("合法纠偏应提交") };
        mounted.settle();
        assert_eq!(query.auction_parameter().selected_id(), Some(AUCTION));
        assert_eq!(*mounted.view.query.borrow(), query);
        assert_eq!(
            mounted.dom.in_runtime(|| {
                mounted
                    .actions()
                    .choose_auction
                    .call(AuctionSelection::Automatic(Some(AUCTION.to_owned())))
            }),
            QueryCommit::Unchanged(query)
        );
        assert_eq!(mounted.pushes.get(), 0);
    }

    #[test]
    fn filters_and_pages_commit_only_after_a_changed_address_is_written() {
        let mut mounted = Mounted::new("/history");
        assert!(!mounted.dom.in_runtime(|| mounted.actions().apply.call((None, 1))));
        assert_eq!(mounted.history.current_route(), "/history");
        assert_eq!(mounted.pushes.get(), 0);

        assert!(mounted.dom.in_runtime(|| mounted.actions().apply.call((Some("Phase Robe"), 1))));
        mounted.settle();
        let first = mounted.history.current_route();
        assert_eq!(*mounted.view.address.borrow(), first);
        assert_eq!(mounted.view.query.borrow().history_name.as_deref(), Some("Phase Robe"));
        assert!(!mounted.dom.in_runtime(|| mounted.actions().apply.call((Some("Phase Robe"), 1))));
        assert_eq!(mounted.pushes.get(), 1);

        mounted.rejects_writes.set(true);
        assert!(
            !mounted.dom.in_runtime(|| mounted.actions().apply.call((Some("Shade Helmet"), 2)))
        );
        mounted.settle();
        assert_eq!(mounted.history.current_route(), first);
        assert_eq!(*mounted.view.address.borrow(), first);
        assert_eq!(mounted.view.query.borrow().history_name.as_deref(), Some("Phase Robe"));
        assert_eq!(mounted.view.query.borrow().page, 1);

        mounted.rejects_writes.set(false);
        assert!(mounted.dom.in_runtime(|| mounted.actions().apply.call((Some("Phase Robe"), 2))));
        mounted.settle();
        assert!(mounted.dom.in_runtime(|| mounted.actions().apply.call((Some("Shade Helmet"), 2))));
        mounted.settle();
        assert_eq!(mounted.pushes.get(), 3);
        for page in [2, 1] {
            mounted.dom.in_runtime(|| mounted.actions().back.call(()));
            mounted.settle();
            assert_eq!(*mounted.view.address.borrow(), mounted.history.current_route());
            assert_eq!(mounted.view.query.borrow().history_name.as_deref(), Some("Phase Robe"));
            assert_eq!(mounted.view.query.borrow().page, page);
        }
        assert_eq!(mounted.history.current_route(), first);
    }
}
