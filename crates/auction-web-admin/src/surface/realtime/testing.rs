//! 宿主筛选回归的历史、表单事件与 GET 记录。

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use auction_web_shared::build_bundle;
use dioxus::dioxus_core::{AttributeValue, ElementId, Mutation};
use dioxus::html;
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
use url::form_urlencoded;

use crate::surface::ADMIN_LOCALE_CATALOG;

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
        html::set_event_converter(Box::new(FormConverter));
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
        let data = html::PlatformEventData::new(Box::new(value.to_owned()));
        let event: Event<dyn Any> = Event::new(Rc::new(data), true);
        self.dom.runtime().handle_event(name, event, target);
        self.render()
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

struct FormValue(String);

impl html::HasFileData for FormValue {
    fn files(&self) -> Vec<html::FileData> {
        Vec::new()
    }
}

impl html::HasFormData for FormValue {
    fn value(&self) -> String {
        self.0.clone()
    }
    fn valid(&self) -> bool {
        true
    }
    fn values(&self) -> Vec<(String, html::FormValue)> {
        Vec::new()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// 宿主测试使用实际 Dioxus 表单事件，替身只承接控件值，无需启用额外的序列化 feature。
struct FormConverter;

macro_rules! unused_events {
    ($($method:ident: $data:ident),+ $(,)?) => {
        $(fn $method(&self, _: &html::PlatformEventData) -> html::$data {
            panic!("筛选测试未预期的事件：{}", stringify!($method))
        })+
    };
}

impl html::HtmlEventConverter for FormConverter {
    fn convert_form_data(&self, event: &html::PlatformEventData) -> html::FormData {
        html::FormData::new(FormValue(event.downcast::<String>().unwrap().clone()))
    }

    unused_events!(
        convert_animation_data: AnimationData,
        convert_cancel_data: CancelData,
        convert_clipboard_data: ClipboardData,
        convert_composition_data: CompositionData,
        convert_drag_data: DragData,
        convert_focus_data: FocusData,
        convert_image_data: ImageData,
        convert_keyboard_data: KeyboardData,
        convert_media_data: MediaData,
        convert_mounted_data: MountedData,
        convert_mouse_data: MouseData,
        convert_pointer_data: PointerData,
        convert_resize_data: ResizeData,
        convert_scroll_data: ScrollData,
        convert_selection_data: SelectionData,
        convert_toggle_data: ToggleData,
        convert_touch_data: TouchData,
        convert_transition_data: TransitionData,
        convert_visible_data: VisibleData,
        convert_wheel_data: WheelData,
    );
}
