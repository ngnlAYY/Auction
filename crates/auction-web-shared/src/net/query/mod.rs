//! URL query 参数与组件状态的双向同步。
//! 只读写当前地址的 query 段，不接管路由。多键共存靠「声明各自管理的键」：写回只替换本
//! hook 声明的键，其余键（其他 hook 或路由持有）原样保留。状态形状与解析/序列化规则由
//! 调用方注入，本模块不含业务键名。

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dioxus::dioxus_core::Runtime;
use dioxus::history::history;
use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

/// 写回地址栏时创建还是替换浏览器历史项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryWriteMode {
    /// 用户主动切换筛选/分页：创建可返回的浏览器历史项。
    Push,
    /// 程序化纠偏（canonical 化、越界钳制等）：替换当前项，不污染历史栈。
    Replace,
}

/// 由 surface 路由提供的同页地址读写入口。写入返回 `true` 时，路由也已收到新地址。
pub trait QueryLocation {
    /// 当前根相对地址，包含查询和片段。
    fn current(&self) -> String;

    /// 按给定模式提交同页地址，保留视口位置。
    fn write(&self, location: &str, mode: QueryWriteMode) -> bool;
}

/// 取得当前组件所属路由的地址读写入口，供该 surface 的查询状态复用。
pub fn query_location() -> Option<Rc<dyn QueryLocation>> {
    Runtime::try_current()?.try_current_scope_id()?;
    try_consume_context()
}

#[derive(Clone)]
struct QueryRouter {
    current: Rc<dyn Fn() -> String>,
    observed: Rc<RefCell<QueryNavigation>>,
}

#[derive(Default)]
struct QueryNavigation {
    location: String,
    revision: u64,
}

impl QueryRouter {
    fn observe(&self, location: &str) -> u64 {
        let mut observed = self.observed.borrow_mut();
        let (previous_path, previous_search, _) = location_parts(&observed.location);
        let (path, search, _) = location_parts(location);
        if path != previous_path || search != previous_search {
            observed.location = location.to_owned();
            observed.revision = observed.revision.wrapping_add(1);
        }
        observed.revision
    }
}

fn query_router() -> Option<QueryRouter> {
    Runtime::try_current()?.try_current_scope_id()?;
    try_consume_context()
}

/// 让后代查询状态订阅 surface 路由的位置变化。读取函数须在响应式作用域内订阅路由，并返回完整地址。
pub fn use_query_router(current: impl Fn() -> String + 'static) {
    use_context_provider(|| QueryRouter { current: Rc::new(current), observed: Rc::default() });
}

/// 以首个分隔符拆出路径、查询和片段，查询和片段各自保留其分隔符。
pub fn location_parts(location: &str) -> (&str, &str, &str) {
    let (path_query, fragment) = location.split_at(location.find('#').unwrap_or(location.len()));
    let (path, search) = path_query.split_at(path_query.find('?').unwrap_or(path_query.len()));
    (path, search, fragment)
}

/// 从完整 search 中只剥掉第一个问号，其余问号属于查询键或值。
pub fn parse_query_pairs(search: &str) -> impl Iterator<Item = (Cow<'_, str>, Cow<'_, str>)> {
    let raw = search.strip_prefix('?').unwrap_or_default();
    url::form_urlencoded::parse(raw.as_bytes())
}

/// 读取当前地址栏的全部 query 键值对；没有路由上下文的非 WASM 构建返回空列表。
pub fn read_query_pairs() -> Vec<(String, String)> {
    if let Some(location) = query_location() {
        return owned_query_pairs(location_parts(&location.current()).1);
    }
    cfg_select! {
        target_arch = "wasm32" => {
            let Some(window) = web_sys::window() else {
                return Vec::new();
            };
            let search = window.location().search().unwrap_or_default();
            owned_query_pairs(&search)
        }
        _ => Vec::new(),
    }
}

fn owned_query_pairs(search: &str) -> Vec<(String, String)> {
    parse_query_pairs(search).map(|(key, value)| (key.into_owned(), value.into_owned())).collect()
}

/// 把 `pairs` 合并写回当前地址的 query 段。
/// `keys` 声明本次写回管理的键：这些键的旧值整体替换为 `pairs`，其余键原样保留。
/// 返回地址栏写入是否成功。没有路由上下文的非 WASM 构建返回 `false`。
pub fn write_query_pairs(keys: &[&str], pairs: &[(String, String)], mode: QueryWriteMode) -> bool {
    if let Some(router) = query_router() {
        let history = history();
        let before = history.current_route();
        if !write_query_pairs_to_history(keys, pairs, mode) {
            return false;
        }
        // 先记下尚未恢复的导航，再把成功的页内写回记为同一导航下的最新地址。
        // 分页与筛选共享这份地址，拒写时两者的去重基准都不变。
        router.observe(&before);
        router.observed.borrow_mut().location = history.current_route();
        return true;
    }
    write_query_pairs_to_history(keys, pairs, mode)
}

fn write_query_pairs_to_history(
    keys: &[&str],
    pairs: &[(String, String)],
    mode: QueryWriteMode,
) -> bool {
    if let Some(location) = query_location() {
        return location.write(&patch_query(&location.current(), keys, pairs, false), mode);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return false;
        };
        let Ok(history) = window.history() else {
            return false;
        };
        let location = window.location();
        let pathname = location.pathname().unwrap_or_else(|_| "/".to_owned());
        let search = location.search().unwrap_or_default();
        let hash = location.hash().unwrap_or_default();
        let current = format!("{pathname}{search}{hash}");
        let target = patch_query(&current, keys, pairs, false);
        let state = history.state().unwrap_or(JsValue::NULL);
        write_history_entry(&history, &state, Some(&target), mode)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (keys, pairs, mode);
        false
    }
}

/// 经 surface 路由替换指定查询键，连同空段保留其余查询片段。没有路由上下文时拒绝写入。
pub fn write_query_pairs_preserving_segments(
    keys: &[&str],
    pairs: &[(String, String)],
    mode: QueryWriteMode,
) -> bool {
    query_location().is_some_and(|location| {
        location.write(&patch_query(&location.current(), keys, pairs, true), mode)
    })
}

fn patch_query(
    location: &str,
    keys: &[&str],
    pairs: &[(String, String)],
    preserve_empty: bool,
) -> String {
    let (path, search, hash) = location_parts(location);
    let raw = search.strip_prefix('?').unwrap_or_default();
    let mut fields: Vec<_> = raw
        .split('&')
        .filter(|field| {
            (preserve_empty && !raw.is_empty() && field.is_empty())
                || url::form_urlencoded::parse(field.as_bytes())
                    .next()
                    .is_some_and(|(key, _)| !keys.contains(&key.as_ref()))
        })
        .collect();
    let encoded = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
    if !encoded.is_empty() {
        fields.push(&encoded);
    }
    if fields.is_empty() {
        format!("{path}{hash}")
    } else {
        format!("{path}?{}{hash}", fields.join("&"))
    }
}

/// 向浏览器提交历史项。调用方在成功后才发布地址和查询状态。
#[cfg(target_arch = "wasm32")]
pub fn write_history_entry(
    history: &web_sys::History,
    state: &JsValue,
    url: Option<&str>,
    mode: QueryWriteMode,
) -> bool {
    match mode {
        QueryWriteMode::Push => history.push_state_with_url(state, "", url).is_ok(),
        QueryWriteMode::Replace => history.replace_state_with_url(state, "", url).is_ok(),
    }
}

/// 状态 → 写回键值对的序列化器（省略等于默认值的键）。
type SerializeQuery<T> = dyn Fn(&T) -> Vec<(String, String)>;

/// 全部 query 键值对 → 状态的解析器（挂载初值与导航恢复共用）。
type ParseQuery<T> = dyn Fn(&[(String, String)]) -> T;

/// [`use_query_state`] 返回的句柄：一个随地址栏双向同步的状态信号。
#[derive(Clone)]
pub struct QueryState<T: 'static> {
    /// 当前状态；挂载、同页导航与浏览器前进/后退时取自地址栏。
    pub state: Signal<T>,
    keys: &'static [&'static str],
    serialize: Rc<SerializeQuery<T>>,
    router: Option<QueryRouter>,
    applied_revision: Rc<Cell<u64>>,
}

impl<T: 'static> QueryState<T> {
    /// 更新状态并按 `mode` 写回地址栏（只替换本 hook 声明管理的键）。
    pub fn set(&mut self, value: T, mode: QueryWriteMode) {
        if write_query_pairs(self.keys, &(self.serialize)(&value), mode)
            && let Some(router) = &self.router
        {
            // 成功写回后，本实例会直接提交对应状态，无需再恢复同一次导航。
            self.applied_revision.set(router.observed.borrow().revision);
        }
        self.state.set(value);
    }

    /// 用户主动变更：更新状态并创建可返回的浏览器历史项。
    pub fn push(&mut self, value: T) {
        self.set(value, QueryWriteMode::Push);
    }

    /// 程序化纠偏：更新状态并替换当前历史项。
    pub fn replace(&mut self, value: T) {
        self.set(value, QueryWriteMode::Replace);
    }
}

/// 挂载与 URL query 双向同步的状态。
///
/// `keys` 是本 hook 管理的 query 键，写回只替换这些键；挂载与导航恢复共用 `parse`，
/// `serialize` 输出键值对并省略等于默认值的键。路由由 surface 提供，没有路由时监听浏览器前进/后退。
pub fn use_query_state<T: Clone + PartialEq + 'static>(
    keys: &'static [&'static str],
    parse: impl Fn(&[(String, String)]) -> T + 'static,
    serialize: impl Fn(&T) -> Vec<(String, String)> + 'static,
) -> QueryState<T> {
    let parse = use_hook(|| Rc::new(parse) as Rc<ParseQuery<T>>);
    let serialize = use_hook(|| Rc::new(serialize) as Rc<SerializeQuery<T>>);
    let route = use_hook(try_consume_context::<QueryRouter>);
    let initial = use_hook(|| {
        route.as_ref().map(|router| {
            let location = (router.current)();
            let revision = router.observe(&location);
            (location, revision)
        })
    });
    let state = use_signal(|| {
        let pairs = initial.as_ref().map_or_else(read_query_pairs, |(location, _)| {
            owned_query_pairs(location_parts(location).1)
        });
        parse(&pairs)
    });
    // 解析器与原生事件都在创建查询状态的组件作用域内运行。
    let restore = use_callback({
        let mut state = state;
        move |pairs: Vec<(String, String)>| {
            let value = parse(&pairs);
            if value != *state.peek() {
                state.set(value);
            }
        }
    });
    let start_revision = initial.as_ref().map_or(0, |(_, revision)| *revision);
    let applied_revision = use_hook(|| Rc::new(Cell::new(start_revision)));
    let applied = Rc::clone(&applied_revision);
    let watched_route = route.clone();
    #[cfg(target_arch = "wasm32")]
    let popstate_fallback = route.is_none();
    use_effect(move || {
        let (Some(router), Some((initial, _))) = (&watched_route, &initial) else { return };
        let location = (router.current)();
        let revision = router.observe(&location);
        let (path, search, _) = location_parts(&location);
        // 跨页后由新页面恢复地址。同址通知和片段变化沿用已应用的查询。
        if path == location_parts(initial).0 && applied.replace(revision) != revision {
            restore.call(owned_query_pairs(search));
        }
    });
    #[cfg(target_arch = "wasm32")]
    use_popstate_listener(
        use_callback(move |()| restore.call(read_query_pairs())),
        popstate_fallback,
    );

    QueryState { state, keys, serialize, router: route, applied_revision }
}

/// 浏览器前进/后退时递增调用方持有的位置版本；组件卸载时移除监听。
/// surface 只按版本变化重读自身状态，本 hook 不解析地址，也不接管路由。非 WASM 构建为空操作。
pub fn use_popstate_revision(revision: Signal<u64>) {
    #[cfg(target_arch = "wasm32")]
    {
        let mut revision = revision;
        use_popstate_listener(
            use_callback(move |()| {
                revision.set(revision().saturating_add(1));
            }),
            true,
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = revision;
}

/// 每个组件只安装一次 popstate 监听，卸载时由守卫移除；回调经 `Callback` 恢复创建组件的 Scope。
#[cfg(target_arch = "wasm32")]
fn use_popstate_listener(on_popstate: Callback<()>, enabled: bool) {
    let guard = use_hook(|| Rc::new(RefCell::new(None::<PopstateGuard>)));
    let cleanup_guard = Rc::clone(&guard);
    use_drop(move || {
        if let Some(guard) = cleanup_guard.borrow_mut().take() {
            guard.cleanup();
        }
    });
    use_effect(move || {
        // effect 无响应式依赖不会重跑，守卫已存在时直接返回。
        if !enabled || guard.borrow().is_some() {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let callback = Closure::<dyn FnMut()>::new(move || {
            on_popstate.call(());
        });
        if window
            .add_event_listener_with_callback("popstate", callback.as_ref().unchecked_ref())
            .is_ok()
        {
            guard.replace(Some(PopstateGuard { window, callback }));
        }
    });
}

/// popstate 监听的生命周期守卫：组件卸载时移除监听并释放闭包。
#[cfg(target_arch = "wasm32")]
struct PopstateGuard {
    window: web_sys::Window,
    callback: Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl PopstateGuard {
    fn cleanup(self) {
        let _ = self.window.remove_event_listener_with_callback(
            "popstate",
            self.callback.as_ref().unchecked_ref(),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use dioxus::dioxus_core;
    use dioxus::history::{History, MemoryHistory};
    use dioxus::prelude::component;
    use dioxus::prelude::dioxus_signals;
    use dioxus::prelude::{
        Callback, Element, Signal, VirtualDom, consume_context, rsx, use_callback, use_signal,
    };
    use dioxus::prelude::{ReadableExt, WritableExt};

    use super::{
        QueryLocation, QueryWriteMode, location_parts, owned_query_pairs, patch_query,
        use_query_router, use_query_state, write_query_pairs,
    };

    #[test]
    fn shared_query_patch_preserves_literal_question_marks_and_fragment() {
        let size = [("page_size".to_owned(), "50".to_owned())];
        let next = patch_query("/??page=2&ref=x#title", &["page", "page_size"], &size, false);
        assert_eq!(next, "/??page=2&ref=x&page_size=50#title");
        assert_eq!(
            owned_query_pairs(location_parts(&next).1),
            [
                ("?page".to_owned(), "2".to_owned()),
                ("ref".to_owned(), "x".to_owned()),
                size[0].clone()
            ]
        );
        assert_eq!(
            patch_query("/?page=2#title", &["page", "page_size"], &size, false),
            "/?page_size=50#title"
        );
    }

    #[test]
    fn scoped_patch_preserves_other_raw_segments_including_empty_ones() {
        let initial = "/?world=main&%74ype=%63lo&&keep=%2b%20x&keep=#lots";
        let pairs = [("world".to_owned(), "isekai".to_owned())];
        assert_eq!(
            patch_query(initial, &["world"], &pairs, true),
            "/?%74ype=%63lo&&keep=%2b%20x&keep=&world=isekai#lots",
        );
        assert_eq!(
            patch_query(initial, &["world"], &pairs, false),
            "/?%74ype=%63lo&keep=%2b%20x&keep=&world=isekai#lots",
        );
        assert_eq!(patch_query("/#lots", &["world"], &pairs, true), "/?world=isekai#lots");
    }

    struct Browser {
        history: Rc<MemoryHistory>,
        rejects_writes: Cell<bool>,
        route: Cell<Option<Signal<String>>>,
        apply: Cell<Option<Callback<(QueryWriteMode, &'static str)>>>,
        actor: RefCell<String>,
        parsed: RefCell<Vec<String>>,
    }

    impl Browser {
        fn notify(&self) {
            self.route.get().unwrap().set(self.history.current_route());
        }
    }

    impl QueryLocation for Browser {
        fn current(&self) -> String {
            self.history.current_route()
        }

        fn write(&self, location: &str, mode: QueryWriteMode) -> bool {
            if self.rejects_writes.get() {
                return false;
            }
            match mode {
                QueryWriteMode::Push => self.history.push(location.to_owned()),
                QueryWriteMode::Replace => self.history.replace(location.to_owned()),
            }
            self.notify();
            true
        }
    }

    fn query_root() -> Element {
        let browser = consume_context::<Rc<Browser>>();
        let route = use_signal(|| browser.current());
        browser.route.set(Some(route));
        use_query_router(move || route.read().clone());
        rsx! { ActorQuery {} }
    }

    #[component]
    fn ActorQuery() -> Element {
        let browser = consume_context::<Rc<Browser>>();
        let query = use_query_state(
            &["actor"],
            {
                let browser = browser.clone();
                move |pairs| {
                    let actor = pairs
                        .iter()
                        .find(|(key, _)| key == "actor")
                        .map_or_else(String::new, |(_, value)| value.clone());
                    browser.parsed.borrow_mut().push(actor.clone());
                    actor
                }
            },
            |actor| {
                if actor.is_empty() {
                    Vec::new()
                } else {
                    vec![("actor".to_owned(), actor.clone())]
                }
            },
        );
        browser.actor.replace(query.state.read().clone());
        browser.apply.set(Some(use_callback(
            move |(mode, actor): (QueryWriteMode, &'static str)| {
                query.clone().set(actor.to_owned(), mode);
            },
        )));
        rsx! {}
    }

    #[test]
    fn same_page_navigation_restores_without_reparsing_writes_or_other_paths() {
        let browser = Rc::new(Browser {
            history: Rc::new(MemoryHistory::with_initial_path("/audit?actor=alice")),
            rejects_writes: Cell::new(false),
            route: Cell::new(None),
            apply: Cell::new(None),
            actor: RefCell::new(String::new()),
            parsed: RefCell::new(Vec::new()),
        });
        let mut dom = VirtualDom::new(query_root)
            .with_root_context(browser.clone())
            .with_root_context(browser.clone() as Rc<dyn QueryLocation>)
            .with_root_context(browser.history.clone() as Rc<dyn History>);
        dom.rebuild_in_place();
        let mut step = |action: &dyn Fn(), expected| {
            dom.in_runtime(action);
            for _ in 0..3 {
                dom.render_immediate_to_vec();
            }
            assert_eq!(browser.actor.borrow().as_str(), expected);
        };
        let apply = |mode, actor| browser.apply.get().unwrap().call((mode, actor));
        let navigate = |address, mode| assert!(browser.write(address, mode));
        step(&|| {}, "alice");
        step(&|| apply(QueryWriteMode::Push, "bob"), "bob");
        assert_eq!(browser.current(), "/audit?actor=bob");
        step(&|| apply(QueryWriteMode::Replace, "carol"), "carol");
        assert_eq!(browser.current(), "/audit?actor=carol");
        step(&|| navigate("/audit", QueryWriteMode::Push), "");
        step(
            &|| {
                browser.history.go_back();
                browser.notify();
            },
            "carol",
        );
        step(
            &|| {
                browser.history.go_forward();
                browser.notify();
            },
            "",
        );

        browser.rejects_writes.set(true);
        step(&|| apply(QueryWriteMode::Push, "dave"), "dave");
        assert_eq!(browser.current(), "/audit");
        browser.rejects_writes.set(false);
        step(&|| navigate("/audit", QueryWriteMode::Push), "dave");
        step(&|| navigate("/audit#filters", QueryWriteMode::Replace), "dave");
        step(&|| navigate("/users?actor=eve", QueryWriteMode::Push), "dave");
        browser.history.push("/mails?actor=mallory".to_owned());
        step(
            &|| {
                browser.history.go_back();
                browser.notify();
            },
            "dave",
        );
        step(
            &|| {
                browser.history.go_back();
                browser.notify();
            },
            "",
        );
        assert_eq!(&*browser.parsed.borrow(), &["alice", "", "carol", "", ""]);
    }

    #[test]
    fn same_page_navigation_before_page_size_write_clears_applied_actor() {
        let browser = Rc::new(Browser {
            history: Rc::new(MemoryHistory::with_initial_path("/audit?actor=alice")),
            rejects_writes: Cell::new(false),
            route: Cell::new(None),
            apply: Cell::new(None),
            actor: RefCell::new(String::new()),
            parsed: RefCell::new(Vec::new()),
        });
        let mut dom = VirtualDom::new(query_root)
            .with_root_context(browser.clone())
            .with_root_context(browser.clone() as Rc<dyn QueryLocation>)
            .with_root_context(browser.history.clone() as Rc<dyn History>);
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        assert_eq!(browser.actor.borrow().as_str(), "alice");

        dom.in_scope(dioxus_core::ScopeId::APP, || {
            assert!(browser.write("/audit", QueryWriteMode::Push));
            assert!(write_query_pairs(
                &["page_size"],
                &[("page_size".to_owned(), "20".to_owned())],
                QueryWriteMode::Replace,
            ));
        });
        for _ in 0..3 {
            dom.render_immediate_to_vec();
        }
        assert_eq!(browser.current(), "/audit?page_size=20");
        assert_eq!(browser.actor.borrow().as_str(), "");
    }
}
