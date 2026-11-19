//! Public 登录返回地址的编码、解析与启动快照。

use auction_web_shared::query::{location_parts, parse_query_pairs};
use dioxus::prelude::*;
use url::form_urlencoded;

/// 在路由首次渲染期间共享的同源来源，首次渲染完成后由路由清除。
#[derive(Clone, Copy)]
pub(crate) struct InitialLoginReturnLocation(Signal<Option<String>>);

impl InitialLoginReturnLocation {
    fn new(value: Option<&str>) -> Self {
        Self(Signal::new(Some(safe_return_path(value))))
    }

    fn for_pathname(&self, pathname: &str) -> Option<String> {
        self.0.read().as_ref().filter(|value| location_parts(value).0 == pathname).cloned()
    }
}

/// 在 Router 规范化地址前保存首次同源 pathname + search。
pub(crate) fn initial_login_return_location() -> InitialLoginReturnLocation {
    InitialLoginReturnLocation::new(browser_path_and_search().as_deref())
}

/// 等 Router 首次渲染完成才作废快照，之后不能用旧筛选覆盖同一路径的新地址。
pub(crate) fn use_expire_initial_login_return_location() {
    let mut initial = use_context::<InitialLoginReturnLocation>().0;
    use_effect(move || initial.set(None));
}

/// 为需要认证的 CTA 保存当前 pathname 与 search；直接登录入口不携带返回地址。
pub(crate) fn login_href_for_current_location() -> String {
    let Some(return_to) = current_path_and_search() else {
        return "/login".to_owned();
    };
    // 旧版用 Router state 保存来源；根路径本就是登录页默认返回值，地址栏保持裸 `/login`。
    if return_to == "/" {
        return "/login".to_owned();
    }
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("return_to", &return_to)
        .finish();
    format!("/login?{query}")
}

fn safe_return_path(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "/".to_owned();
    };
    if !value.starts_with('/')
        || value.starts_with("//")
        || value.starts_with("/login?")
        || value == "/login"
        || value.contains(['\\', '#'])
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return "/".to_owned();
    }
    value.to_owned()
}

/// 读取并校验登录成功后的站内返回地址。
pub(super) fn login_return_path() -> String {
    let location = router().full_route_string();
    let (_, search, _) = location_parts(&location);
    // 先拆外层参数再解码一次，保留返回地址内部的分隔符与原有百分号编码。
    let target = parse_query_pairs(search)
        .filter_map(|(key, value)| (key == "return_to").then_some(value))
        .last();
    safe_return_path(target.as_deref())
}

fn current_path_and_search() -> Option<String> {
    let current = browser_path_and_search()?;
    if !current.contains('?')
        && let Some(initial) = try_consume_context::<InitialLoginReturnLocation>()
        && let Some(source) = initial.for_pathname(&current)
    {
        return Some(source);
    }
    (safe_return_path(Some(&current)) == current).then_some(current)
}

fn browser_path_and_search() -> Option<String> {
    if let Some(router) = dioxus::router::try_router() {
        // 订阅完整地址，同页筛选更新后已挂载的入口也会重算 href。
        let location = router.full_route_string();
        let (path, search, _) = location_parts(&location);
        return Some(format!("{path}{search}"));
    }
    cfg_select! {
        target_arch = "wasm32" => {
            let location = web_sys::window()?.location();
            let pathname = location.pathname().ok()?;
            let search = location.search().unwrap_or_default();
            Some(format!("{pathname}{search}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use dioxus::history::MemoryHistory;
    use dioxus::prelude::*;
    use url::form_urlencoded;

    use super::{
        InitialLoginReturnLocation, login_href_for_current_location, login_return_path,
        use_expire_initial_login_return_location,
    };

    #[derive(Clone, PartialEq, Routable)]
    enum ReturnRoute {
        #[route("/")]
        Source {},
        #[route("/bidlog")]
        BidLogSource {},
        #[route("/login?:return_to")]
        Destination { return_to: Option<String> },
    }

    #[derive(Default)]
    struct ReturnControls {
        href: RefCell<String>,
        navigate: Cell<Option<Callback<String>>>,
        complete_login: Cell<Option<Callback<()>>>,
    }

    #[component]
    fn Source() -> Element {
        let controls = use_context::<Rc<ReturnControls>>();
        controls.href.replace(login_href_for_current_location());
        controls.navigate.set(Some(use_callback(|address: String| {
            navigator().push(address);
        })));
        rsx! {}
    }

    #[component]
    fn BidLogSource() -> Element {
        rsx! { Source {} }
    }

    #[component]
    fn Destination(return_to: Option<String>) -> Element {
        let _ = return_to;
        let controls = use_context::<Rc<ReturnControls>>();
        let target = login_return_path();
        controls.complete_login.set(Some(use_callback(move |()| {
            navigator().replace(target.clone());
        })));
        rsx! {}
    }

    fn return_harness() -> Element {
        let startup = use_context::<String>();
        use_context_provider(|| InitialLoginReturnLocation::new(Some(&startup)));
        use_expire_initial_login_return_location();
        rsx! { Router::<ReturnRoute> {} }
    }

    fn mounted_return(
        initial: &str,
        snapshot: &str,
    ) -> (VirtualDom, Rc<MemoryHistory>, Rc<ReturnControls>) {
        let history = Rc::new(MemoryHistory::with_initial_path(initial));
        let controls = Rc::new(ReturnControls::default());
        let mut dom = VirtualDom::new(return_harness)
            .with_root_context(history.clone() as Rc<dyn History>)
            .with_root_context(controls.clone())
            .with_root_context(snapshot.to_owned());
        dom.rebuild_in_place();
        (dom, history, controls)
    }

    fn settle_return(dom: &mut VirtualDom) {
        for _ in 0..3 {
            dom.process_events();
            dom.render_immediate_to_vec();
        }
    }

    #[test]
    fn login_returns_to_the_exact_nested_query_from_the_generated_link() {
        let source = "/bidlog?world=main&page=2&auction=67e55044-10b1-426f-9247-bb680e5fe0c8&name=A%26B%3DC%25%2B%E5%BC%93";
        let (mut dom, history, controls) = mounted_return("/bidlog", "/");
        dom.in_runtime(|| controls.navigate.get().unwrap().call(source.to_owned()));
        settle_return(&mut dom);

        let href = controls.href.borrow().clone();
        let (_, search) = href.split_once('?').unwrap();
        assert_eq!(
            form_urlencoded::parse(search.as_bytes()).collect::<Vec<_>>(),
            [("return_to".into(), source.into())]
        );
        dom.in_runtime(|| controls.navigate.get().unwrap().call(href));
        settle_return(&mut dom);
        dom.in_runtime(|| controls.complete_login.get().unwrap().call(()));
        settle_return(&mut dom);
        assert_eq!(history.current_route(), source);
    }

    #[test]
    fn login_link_does_not_reuse_the_startup_query_after_leaving_it() {
        let initial = "/?world=isekai&page=3";
        let (mut dom, history, controls) = mounted_return("/", initial);
        assert_eq!(
            controls.href.borrow().as_str(),
            "/login?return_to=%2F%3Fworld%3Disekai%26page%3D3"
        );
        settle_return(&mut dom);
        assert_eq!(controls.href.borrow().as_str(), "/login");

        for address in [initial, "/"] {
            dom.in_runtime(|| controls.navigate.get().unwrap().call(address.to_owned()));
            settle_return(&mut dom);
        }
        assert_eq!(history.current_route(), "/");
        assert_eq!(controls.href.borrow().as_str(), "/login");
    }

    #[test]
    fn login_rejects_unsafe_and_disallowed_return_paths_after_decoding() {
        for target in [
            "https://example.test/",
            "//example.test/",
            "javascript:alert(1)",
            "/\\example.test/",
            "/bidlog?name=a\nb",
            "/bidlog?name=a\u{7f}",
            "/me#login-duration",
            "/login?return_to=%2Fme",
        ] {
            let (mut dom, history, controls) = mounted_return("/", "/");
            let query = form_urlencoded::Serializer::new(String::new())
                .append_pair("return_to", target)
                .finish();
            dom.in_runtime(|| controls.navigate.get().unwrap().call(format!("/login?{query}")));
            settle_return(&mut dom);
            dom.in_runtime(|| controls.complete_login.get().unwrap().call(()));
            settle_return(&mut dom);
            assert_eq!(history.current_route(), "/", "{target:?}");
        }
    }
}
