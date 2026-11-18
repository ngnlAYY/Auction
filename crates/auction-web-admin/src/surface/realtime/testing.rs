//! 宿主筛选回归的历史、表单事件与 GET 记录。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use auction_web_shared::build_bundle;
use dioxus::dioxus_core::{AttributeValue, ElementId, Mutation};
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
use url::form_urlencoded;

use crate::surface::ADMIN_LOCALE_CATALOG;
use crate::surface::pending::use_pending_command_host;
use crate::surface::test_events::{dispatch_click_event, dispatch_form_event};

type QueryPairs = Vec<(String, String)>;

#[derive(Default)]
struct TestHistory {
    initial: QueryPairs,
    restore: Cell<Option<Callback<QueryPairs>>>,
    address: Cell<Option<Callback<(), QueryPairs>>>,
}

pub(in crate::surface) fn use_test_history<T: PartialEq + 'static>(
    mut state: Signal<T>,
    parse: fn(&[(String, String)]) -> T,
    serialize: fn(&T) -> QueryPairs,
) {
    let restore = use_callback(move |pairs: QueryPairs| {
        let next = parse(&pairs);
        if next != *state.peek() {
            state.set(next);
        }
    });
    let address = use_callback(move |()| serialize(&state.peek()));
    use_hook(move || {
        if let Some(history) = try_consume_context::<Rc<TestHistory>>() {
            // 宿主的地址栏以夹具代替，首个资源必须读到书签，而不是先查默认条件。
            state.set(parse(&history.initial));
            history.restore.set(Some(restore));
            history.address.set(Some(address));
        }
    });
}

pub(in crate::surface) struct TestPage {
    dom: VirtualDom,
    history: Rc<TestHistory>,
    gets: Rc<RefCell<Vec<String>>>,
}

impl TestPage {
    pub(in crate::surface) fn mount(
        page: fn() -> Element,
        bookmark: &str,
    ) -> (Self, Vec<Mutation>) {
        let initial = form_urlencoded::parse(bookmark.as_bytes()).into_owned().collect();
        let history = Rc::new(TestHistory { initial, ..TestHistory::default() });
        let gets = Rc::new(RefCell::new(Vec::<String>::new()));
        let dom = VirtualDom::new(test_root)
            .with_root_context(page)
            .with_root_context(history.clone())
            .with_root_context(gets.clone());
        let mut mounted = Self { dom, history, gets };
        let mut edits = mounted.dom.rebuild_to_vec().edits;
        edits.extend(mounted.render());
        (mounted, edits)
    }

    pub(in crate::surface) fn restore(&mut self, pairs: QueryPairs) -> Vec<Mutation> {
        self.history.restore.get().unwrap().call(pairs);
        self.render()
    }

    pub(in crate::surface) fn address(&self) -> QueryPairs {
        self.history.address.get().unwrap().call(())
    }

    pub(in crate::surface) fn take_gets(&self) -> Vec<String> {
        self.gets.take()
    }

    pub(in crate::surface) fn assert_no_gets(&self) {
        let gets = self.take_gets();
        assert!(gets.is_empty(), "未提交有效筛选却发出了 GET：{gets:?}");
    }

    pub(in crate::surface) fn dispatch(
        &mut self,
        name: &str,
        target: ElementId,
        value: &str,
    ) -> Vec<Mutation> {
        if name == "click" {
            dispatch_click_event(&self.dom, target);
        } else {
            dispatch_form_event(&self.dom, target, name, value);
        }
        self.render()
    }

    /// 点击前可用当前树按命令条目等祖先定位按钮。
    pub(in crate::surface) fn dom(&self) -> &VirtualDom {
        &self.dom
    }

    pub(in crate::surface) fn rerender(&mut self) {
        self.dom.mark_dirty(ScopeId::APP);
        self.render();
    }

    fn render(&mut self) -> Vec<Mutation> {
        let mut edits = Vec::new();
        // 地址变化先传播给资源和草稿 effect，随后更新字段子组件。
        for _ in 0..3 {
            edits.extend(self.dom.render_immediate_to_vec().edits);
        }
        edits
    }
}

fn test_root() -> Element {
    let catalog = &ADMIN_LOCALE_CATALOG;
    try_use_init_i18n_from_bundle(|| {
        build_bundle(&catalog.default_locale, catalog.default_ftl, &catalog.default_locale, None)
    })
    .unwrap();
    use_pending_command_host();
    consume_context::<fn() -> Element>()()
}

pub(in crate::surface) fn text_attributes<'a>(
    edits: &'a [Mutation],
    name: &'static str,
) -> impl Iterator<Item = (ElementId, &'a str)> {
    edits.iter().filter_map(move |edit| match edit {
        Mutation::SetAttribute {
            id, name: attribute, value: AttributeValue::Text(value), ..
        } if *attribute == name => Some((*id, value.as_str())),
        _ => None,
    })
}

pub(in crate::surface) fn selected_options(edits: &[Mutation]) -> Vec<&str> {
    text_attributes(edits, "value")
        .filter_map(|(id, value)| {
            edits.iter().any(|edit| matches!(edit,
                Mutation::SetAttribute { id: option, name: "selected", value: AttributeValue::Bool(true), .. }
                    if *option == id
            )).then_some(value)
        })
        .collect()
}

pub(in crate::surface) fn submit_forms(edits: &[Mutation]) -> impl Iterator<Item = ElementId> {
    edits.iter().filter_map(|edit| match edit {
        Mutation::NewEventListener { name, id } if name == "submit" => Some(*id),
        _ => None,
    })
}

pub(in crate::surface) fn get_parameter(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    form_urlencoded::parse(query.as_bytes())
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}
