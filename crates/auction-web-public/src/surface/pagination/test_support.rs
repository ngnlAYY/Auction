//! 宿主页面回归使用的地址、存储和读取边界替身。

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::process::Command;
use std::rc::Rc;

use auction_web_shared::QueryWriteMode;
use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, LocaleSession, build_bundle};
use dioxus::dioxus_core::{
    AttributeValue, DynamicNode, ElementId, Mutation, TemplateAttribute, TemplateNode,
};
use dioxus::html::geometry::{ClientPoint, ElementPoint, PagePoint, ScreenPoint};
use dioxus::html::input_data::{MouseButton, MouseButtonSet};
use dioxus::html::{
    self, FileData, FormData, FormValue, HasFileData, HasFormData, HasMouseData,
    HtmlEventConverter, InteractionElementOffset, InteractionLocation, Modifiers,
    ModifiersInteraction, MouseData, PlatformEventData, PointerInteraction,
};
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

use super::location::query_values;
use super::preferences::parse_public_page_size;
use crate::surface::router::PUBLIC_LOCALE_CATALOG;
use crate::surface::{
    PublicBidRefresh, PublicBoardQuery, PublicContentLoading, normalize_world_query,
};

#[derive(Clone)]
pub(crate) struct PageBrowserFixture(Rc<Browser>);

struct Browser {
    pathname: &'static str,
    pairs: RefCell<Vec<(String, String)>>,
    state: Cell<Option<Signal<PublicBoardQuery>>>,
    write_allowed: Cell<bool>,
    writes: RefCell<Vec<LocationWrite>>,
    sizes: Cell<[i64; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocationWrite {
    pub(crate) keys: Vec<String>,
    pub(crate) pairs: Vec<(String, String)>,
    pub(crate) mode: QueryWriteMode,
    pub(crate) accepted: bool,
}

impl PageBrowserFixture {
    pub(crate) fn new(pathname: &'static str, search: &str) -> Self {
        Self(Rc::new(Browser {
            pathname,
            pairs: RefCell::new(
                url::form_urlencoded::parse(search.as_bytes()).into_owned().collect(),
            ),
            state: Cell::new(None),
            write_allowed: Cell::new(true),
            writes: RefCell::new(Vec::new()),
            sizes: Cell::new([20, 20]),
        }))
    }

    pub(super) fn observe(&self, state: Signal<PublicBoardQuery>) {
        self.0.state.set(Some(state));
    }

    pub(crate) fn state(&self) -> PublicBoardQuery {
        self.signal().peek().clone()
    }

    pub(crate) fn signal(&self) -> Signal<PublicBoardQuery> {
        self.0.state.get().expect("页面查询已挂载")
    }

    pub(super) fn pathname(&self) -> &'static str {
        self.0.pathname
    }

    pub(crate) fn pairs(&self) -> Vec<(String, String)> {
        self.0.pairs.borrow().clone()
    }

    pub(crate) fn value(&self, key: &str) -> Option<String> {
        query_values(&self.0.pairs.borrow(), key).last().map(str::to_owned)
    }

    // 替身只还原测试所需的页面键；原始值供生产筛选器独立校验。
    pub(super) fn query(&self) -> PublicBoardQuery {
        let pairs = self.0.pairs.borrow();
        let value = |key| query_values(&pairs, key).last();
        let page =
            |key| value(key).and_then(|value| value.parse::<i64>().ok()).filter(|page| *page > 0);
        let alias = (self.pathname() == "/bidlog").then(|| page("bidlog_page")).flatten();
        let world = value("world").and_then(normalize_world_query);
        let sizes = self.sizes();
        PublicBoardQuery {
            world,
            status: value("status")
                .filter(|status| matches!(*status, "live" | "final" | "settled"))
                .map(str::to_owned),
            history_name: value("name").map(str::to_owned),
            history_buyer: value("buyer").map(str::to_owned),
            history_seller: value("seller").map(str::to_owned),
            history_sort: value("history_sort").unwrap_or("closed_desc").to_owned(),
            history_group: value("history_group").unwrap_or("equipment").to_owned(),
            bid_log_lot: value("lot").map(str::to_owned),
            page: page("page").or(alias).unwrap_or(1),
            bid_log_page_alias: alias.is_some(),
            page_size: value("page_size").and_then(parse_public_page_size).unwrap_or(sizes[0]),
            movement_page: page("movement_page").unwrap_or(1),
            movement_page_size: value("movement_page_size")
                .and_then(parse_public_page_size)
                .unwrap_or(sizes[1]),
            bid_page: page("bid_page").unwrap_or(1),
            proxy_page: page("proxy_page").unwrap_or(1),
            ..PublicBoardQuery::default()
        }
        .with_auction_values(query_values(&pairs, "auction").map(str::to_owned))
    }

    pub(super) fn write(
        &self,
        keys: &[&str],
        pairs: &[(String, String)],
        mode: QueryWriteMode,
    ) -> bool {
        let accepted = self.0.write_allowed.get();
        self.0.writes.borrow_mut().push(LocationWrite {
            keys: keys.iter().map(|key| (*key).to_owned()).collect(),
            pairs: pairs.to_vec(),
            mode,
            accepted,
        });
        if accepted {
            let mut current = self.0.pairs.borrow_mut();
            current.retain(|(key, _)| !keys.contains(&key.as_str()));
            current.extend_from_slice(pairs);
        }
        accepted
    }

    pub(crate) fn allow_writes(&self, allowed: bool) {
        self.0.write_allowed.set(allowed);
    }

    pub(crate) fn writes(&self) -> Vec<LocationWrite> {
        self.0.writes.borrow().clone()
    }

    pub(crate) fn sizes(&self) -> [i64; 2] {
        self.0.sizes.get()
    }

    pub(super) fn save_size(&self, size: i64, movement: bool) {
        let mut sizes = self.sizes();
        sizes[usize::from(movement)] = size;
        self.0.sizes.set(sizes);
    }
}

pub(crate) struct PageReadFixture<Q, R>(Rc<dyn Fn(Q) -> R>);

impl<Q, R> Clone for PageReadFixture<Q, R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<Q: 'static, R: 'static> PageReadFixture<Q, R> {
    pub(crate) fn new(read: impl Fn(Q) -> R + 'static) -> Self {
        Self(Rc::new(read))
    }

    pub(crate) fn response(query: Q) -> Option<R> {
        try_consume_context::<Self>().map(|fixture| (fixture.0)(query))
    }
}

pub(crate) fn page_test_contexts() {
    let bundle = try_use_init_i18n_from_bundle(|| {
        build_bundle(
            &DEFAULT_PUBLIC_LOCALE,
            PUBLIC_LOCALE_CATALOG.default_ftl,
            &DEFAULT_PUBLIC_LOCALE,
            None,
        )
    })
    .expect("加载测试译文");
    use_context_provider(|| LocaleSession::new(bundle, DEFAULT_PUBLIC_LOCALE));
    use_context_provider(|| Signal::new(0_u64));
    use_context_provider(|| PublicBidRefresh(Signal::new(0_u64)));
    use_context_provider(|| PublicContentLoading(Signal::new(Default::default())));
}

pub(crate) fn settle_page(dom: &mut VirtualDom) -> Vec<Mutation> {
    (0..8).flat_map(|_| dom.render_immediate_to_vec().edits).collect()
}

pub(crate) fn assert_selected_error(
    dom: &VirtualDom,
    edits: &[Mutation],
    value: &str,
    error_id: &str,
    message: &str,
) {
    assert!(edits.iter().any(|edit| matches!(
        edit,
        Mutation::SetAttribute { name: "value", value: AttributeValue::Text(text), id, .. }
            if text == value && edits.iter().any(|other| matches!(
                other,
                Mutation::SetAttribute { name: "selected", value: AttributeValue::Bool(true), id: option_id, .. }
                    if id == option_id
            ))
    )), "错误选项 {value:?} 必须在首帧选中");
    let control = page_event_target(edits, "change", "value", value);
    let description = edits
        .iter()
        .find_map(|edit| match edit {
            Mutation::SetAttribute {
                name: "aria-describedby",
                value: AttributeValue::Text(text),
                id,
                ..
            } if *id == control => Some(text.as_str()),
            _ => None,
        })
        .expect("错误字段引用了描述节点");
    assert_eq!(description, error_id);
    let (vnode, error) =
        find_in_vnode(dom, dom.base_scope().root_node(), &|vnode, node| match node {
            TemplateNode::Element { attrs, .. }
                if text_attribute(vnode, attrs, "id") == Some(description) =>
            {
                Some((vnode, node))
            }
            _ => None,
        })
        .expect("aria-describedby 引用的错误节点存在");
    let expected = format!("{message} {value:?}");
    assert!(
        find_in_node(dom, vnode, error, &|vnode, node| {
            let text = match node {
                TemplateNode::Text { text } => *text,
                TemplateNode::Dynamic { id } => match &vnode.dynamic_nodes[*id] {
                    DynamicNode::Text(text) => &text.value,
                    _ => return None,
                },
                _ => return None,
            };
            text.contains(&expected).then_some(())
        })
        .is_some(),
        "与错误字段关联的文字必须显示原值 {value:?}"
    );
}

pub(crate) fn rendered_event_target(
    dom: &VirtualDom,
    tag: &str,
    name: &str,
    value: &str,
    listener: &str,
) -> ElementId {
    find_in_vnode(dom, dom.base_scope().root_node(), &|vnode, node| {
        let TemplateNode::Element { tag: node_tag, attrs, .. } = node else {
            return None;
        };
        if *node_tag != tag || text_attribute(vnode, attrs, name) != Some(value) {
            return None;
        }
        attrs.iter().find_map(|attr| match attr {
            TemplateAttribute::Dynamic { id }
                if vnode.dynamic_attrs[*id].iter().any(|attr| attr.name == listener) =>
            {
                vnode.mounted_dynamic_attribute(*id, dom)
            }
            _ => None,
        })
    })
    .expect("当前渲染包含实际注册事件的控件")
}

fn text_attribute<'a>(
    vnode: &'a VNode,
    attrs: &[TemplateAttribute],
    name: &str,
) -> Option<&'a str> {
    attrs.iter().find_map(|attr| match attr {
        TemplateAttribute::Static { name: attr_name, value, .. } if *attr_name == name => {
            Some(*value)
        }
        TemplateAttribute::Dynamic { id } => {
            vnode.dynamic_attrs[*id].iter().find_map(|attr| match &attr.value {
                AttributeValue::Text(value) if attr.name == name => Some(value.as_str()),
                _ => None,
            })
        }
        _ => None,
    })
}

fn find_in_vnode<'a, T>(
    dom: &'a VirtualDom,
    vnode: &'a VNode,
    find: &impl Fn(&'a VNode, &'static TemplateNode) -> Option<T>,
) -> Option<T> {
    vnode.template.roots.iter().find_map(|node| find_in_node(dom, vnode, node, find))
}

fn find_in_node<'a, T>(
    dom: &'a VirtualDom,
    vnode: &'a VNode,
    node: &'static TemplateNode,
    find: &impl Fn(&'a VNode, &'static TemplateNode) -> Option<T>,
) -> Option<T> {
    find(vnode, node).or_else(|| match node {
        TemplateNode::Element { children, .. } => {
            children.iter().find_map(|child| find_in_node(dom, vnode, child, find))
        }
        TemplateNode::Dynamic { id } => match &vnode.dynamic_nodes[*id] {
            DynamicNode::Component(component) => {
                let root = component.mounted_scope(*id, vnode, dom)?.try_root_node()?;
                find_in_vnode(dom, root, find)
            }
            DynamicNode::Fragment(nodes) => {
                nodes.iter().find_map(|child| find_in_vnode(dom, child, find))
            }
            DynamicNode::Text(_) | DynamicNode::Placeholder(_) => None,
        },
        TemplateNode::Text { .. } => None,
    })
}

pub(crate) fn run_page_events(module: &str, test: &str, run: impl FnOnce()) {
    // 转换器是整个进程共享的，独立进程避免干扰其他宿主事件测试。
    const CHILD: &str = "AUCTION_PAGINATION_EVENT_TEST";
    if std::env::var_os(CHILD).is_some() {
        html::set_event_converter(Box::new(PageEventConverter));
        run();
        return;
    }
    let (_, module) = module.split_once("::").unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("{module}::{test}"), "--nocapture"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

pub(crate) fn dispatch_page_event(dom: &VirtualDom, name: &str, id: ElementId, value: &str) {
    let data = PlatformEventData::new(Box::new(value.to_owned()));
    dom.runtime().handle_event(name, Event::new(Rc::new(data), true).into_any(), id);
}

pub(crate) fn page_event_target(
    edits: &[Mutation],
    event: &str,
    name: &str,
    value: &str,
) -> ElementId {
    edits.iter().find_map(|edit| match edit {
        Mutation::SetAttribute { name: attr, value: AttributeValue::Text(text), id, .. }
            if *attr == name && text == value
                && edits.iter().any(|edit| matches!(
                    edit, Mutation::NewEventListener { name, id: listener } if *name == event && listener == id
                )) => Some(*id),
        _ => None,
    }).expect("模板包含实际注册事件的控件")
}

struct PageEventConverter;

macro_rules! unsupported {
    ($($convert:ident -> $data:ident),* $(,)?) => {
        $(fn $convert(&self, _event: &PlatformEventData) -> html::$data {
            panic!("该回归只使用表单与鼠标事件");
        })*
    };
}

impl HtmlEventConverter for PageEventConverter {
    fn convert_form_data(&self, event: &PlatformEventData) -> FormData {
        FormData::new(FormValueFixture(event.downcast::<String>().unwrap().clone()))
    }

    fn convert_mouse_data(&self, _event: &PlatformEventData) -> MouseData {
        MouseData::new(Click)
    }

    unsupported!(
        convert_animation_data -> AnimationData, convert_cancel_data -> CancelData,
        convert_clipboard_data -> ClipboardData, convert_composition_data -> CompositionData,
        convert_drag_data -> DragData, convert_focus_data -> FocusData,
        convert_image_data -> ImageData, convert_keyboard_data -> KeyboardData,
        convert_media_data -> MediaData, convert_mounted_data -> MountedData,
        convert_pointer_data -> PointerData, convert_resize_data -> ResizeData,
        convert_scroll_data -> ScrollData, convert_selection_data -> SelectionData,
        convert_toggle_data -> ToggleData, convert_touch_data -> TouchData,
        convert_transition_data -> TransitionData, convert_visible_data -> VisibleData,
        convert_wheel_data -> WheelData,
    );
}

struct FormValueFixture(String);

impl HasFileData for FormValueFixture {
    fn files(&self) -> Vec<FileData> {
        Vec::new()
    }
}

impl HasFormData for FormValueFixture {
    fn value(&self) -> String {
        self.0.clone()
    }
    fn valid(&self) -> bool {
        true
    }
    fn values(&self) -> Vec<(String, FormValue)> {
        Vec::new()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct Click;

impl HasMouseData for Click {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl InteractionLocation for Click {
    fn client_coordinates(&self) -> ClientPoint {
        ClientPoint::zero()
    }
    fn page_coordinates(&self) -> PagePoint {
        PagePoint::zero()
    }
    fn screen_coordinates(&self) -> ScreenPoint {
        ScreenPoint::zero()
    }
}

impl InteractionElementOffset for Click {
    fn element_coordinates(&self) -> ElementPoint {
        ElementPoint::zero()
    }
}

impl ModifiersInteraction for Click {
    fn modifiers(&self) -> Modifiers {
        Modifiers::empty()
    }
}

impl PointerInteraction for Click {
    fn trigger_button(&self) -> Option<MouseButton> {
        Some(MouseButton::Primary)
    }
    fn held_buttons(&self) -> MouseButtonSet {
        MouseButtonSet::empty()
    }
}
