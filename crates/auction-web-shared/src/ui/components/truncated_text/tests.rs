//! 在宿主虚拟 DOM 中挂载截断提示，替换浏览器的测量、焦点与原生文档事件。

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::process::Command;
use std::rc::Rc;

use dioxus::core::{AttributeValue, ElementId, Mutation};
use dioxus::html::geometry::{ClientPoint, ElementPoint, PagePoint, ScreenPoint};
use dioxus::html::input_data::{MouseButton, MouseButtonSet};
use dioxus::html::{
    self, Code, HasKeyboardData, HasMouseData, HtmlEventConverter, InteractionElementOffset,
    InteractionLocation, Location, Modifiers, ModifiersInteraction, PlatformEventData,
    PointerInteraction,
};
use dioxus::prelude::*;

use super::TruncatedText;
use crate::a11y::{ARIA_MODAL_SELECTOR, EscapeUnsubscribe, FOCUSABLE_SELECTOR};

const BODY_ID: &str = "body";
const OUTSIDE_ID: &str = "outside";
const DIALOG_ID: &str = "tooltip-test-dialog";
const LINK_ID: &str = "equipment-link";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ElementKind {
    Plain,
    Control,
    Link,
    AriaDialog,
    NativeDialog { open: bool, modal: bool },
}

#[derive(Clone)]
pub(super) struct DomElement {
    id: String,
    kind: ElementKind,
    parent: Option<Rc<DomElement>>,
    modal_selector_unsupported: bool,
}

impl DomElement {
    fn ancestors(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |element| element.parent.as_deref())
    }

    fn is_focusable(&self) -> bool {
        matches!(self.kind, ElementKind::Link | ElementKind::Control)
    }

    fn matches(&self, selector: &str) -> bool {
        match selector {
            ARIA_MODAL_SELECTOR => self.kind == ElementKind::AriaDialog,
            "dialog:modal" => self.kind == (ElementKind::NativeDialog { open: true, modal: true }),
            "dialog[open]" => matches!(self.kind, ElementKind::NativeDialog { open: true, .. }),
            FOCUSABLE_SELECTOR => self.is_focusable(),
            _ => panic!("测试未实现选择器 {selector}"),
        }
    }

    pub(super) fn closest(&self, selector: &str) -> Result<Option<Self>, ()> {
        if selector == "dialog:modal" && self.modal_selector_unsupported {
            return Err(());
        }
        Ok(self.ancestors().find(|element| element.matches(selector)).cloned())
    }

    pub(super) fn contains(&self, other: &Self) -> bool {
        other.ancestors().any(|element| element.id == self.id)
    }

    pub(super) fn bounding_box(&self) -> (f64, f64, f64) {
        (20.0, 50.0, 80.0)
    }

    pub(super) fn scroll_width(&self) -> i32 {
        120
    }

    pub(super) fn client_width(&self) -> i32 {
        100
    }
}

#[derive(Clone)]
pub(super) struct DomPortal {
    browser: Rc<Browser>,
    id: String,
}

impl DomPortal {
    pub(super) fn remove(self) {
        self.browser.nodes.borrow_mut().remove(&self.id);
        if *self.browser.focused.borrow() == self.id {
            self.browser.focused.replace(BODY_ID.to_owned());
        }
    }
}

type EscapeCallback = Rc<dyn Fn() -> bool>;

#[derive(Default)]
pub(super) struct Browser {
    inside_dialog: bool,
    native_dialog: bool,
    modal_selector_unsupported: bool,
    open_dialog: Cell<Option<Signal<bool>>>,
    nodes: RefCell<BTreeMap<String, DomElement>>,
    focused: RefCell<String>,
    keydown: RefCell<BTreeMap<u32, EscapeCallback>>,
    next_listener: Cell<u32>,
}

impl Browser {
    fn add_element(&self, id: &str, kind: ElementKind, parent: Option<DomElement>) {
        self.nodes.borrow_mut().insert(
            id.to_owned(),
            DomElement {
                id: id.to_owned(),
                kind,
                parent: parent.map(Rc::new),
                modal_selector_unsupported: self.modal_selector_unsupported,
            },
        );
    }

    fn add_control(&self, id: &str, inside_dialog: bool) {
        let parent = inside_dialog.then(|| self.get_element_by_id(DIALOG_ID).unwrap());
        self.add_element(id, ElementKind::Control, parent);
    }

    pub(super) fn get_element_by_id(&self, id: &str) -> Option<DomElement> {
        self.nodes.borrow().get(id).cloned()
    }

    pub(super) fn last_matching(&self, selector: &str) -> Result<Option<DomElement>, ()> {
        if selector == "dialog:modal" && self.modal_selector_unsupported {
            return Err(());
        }
        Ok(self.nodes.borrow().values().rfind(|element| element.matches(selector)).cloned())
    }

    pub(super) fn active_element(&self) -> Option<DomElement> {
        self.get_element_by_id(&self.focused.borrow())
    }

    pub(super) fn focus(&self, element: &DomElement) {
        if element.is_focusable() {
            self.focused.replace(element.id.clone());
        }
    }

    pub(super) fn body(&self) -> Option<DomElement> {
        self.get_element_by_id(BODY_ID)
    }

    pub(super) fn append_portal(self: &Rc<Self>, parent: &DomElement, id: &str) -> DomPortal {
        self.add_element(id, ElementKind::Control, Some(parent.clone()));
        DomPortal { browser: Rc::clone(self), id: id.to_owned() }
    }

    pub(super) fn listen_escape(
        self: Rc<Self>,
        on_escape: impl Fn() -> bool + 'static,
    ) -> EscapeUnsubscribe {
        let id = self.next_listener.get();
        self.next_listener.set(id + 1);
        self.keydown.borrow_mut().insert(id, Rc::new(on_escape));
        Box::new(move || {
            self.keydown.borrow_mut().remove(&id);
        })
    }

    fn dispatch_escape(&self) -> bool {
        let listeners: Vec<_> = self.keydown.borrow().values().cloned().collect();
        listeners.iter().any(|callback| callback())
    }
}

fn scene() -> Element {
    let browser = use_context::<Rc<Browser>>();
    let mut open = use_signal(|| browser.inside_dialog);
    browser.open_dialog.set(Some(open));
    let dialog_id = DIALOG_ID;
    rsx! {
        button { id: OUTSIDE_ID, "Outside" }
        if !browser.inside_dialog {
            TruncatedText { value: "Full equipment name", display: "Equipment…" }
        }
        if open() && browser.native_dialog {
            dialog { id: "{dialog_id}", "Native dialog" }
        } else if open() {
            div {
                id: "{dialog_id}", role: "dialog", aria_modal: "true",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        open.set(false);
                    }
                },
                if browser.inside_dialog {
                    TruncatedText { value: "Full equipment name", display: "Equipment…" }
                }
            }
        }
    }
}

fn equipment_link() -> Element {
    let browser = use_context::<Rc<Browser>>();
    let link_id = LINK_ID;
    let link = rsx! {
        a { id: "{link_id}", href: "/equipment", class: "public-auction-catalog__equipment-link",
            TruncatedText {
                value: "Full equipment name", display: "Equipment…",
                class_name: "ui-equipment-name", portal: true, focusable: false, legacy_dom: true,
            }
        }
    };
    if browser.inside_dialog {
        let dialog_id = DIALOG_ID;
        rsx! { dialog { id: "{dialog_id}", {link} } }
    } else {
        link
    }
}

fn settle(dom: &mut VirtualDom) -> Vec<Mutation> {
    (0..4).flat_map(|_| dom.render_immediate_to_vec().edits).collect()
}

fn element(edits: &[Mutation], matches_id: impl Fn(&str) -> bool) -> (ElementId, &str) {
    edits
        .iter()
        .find_map(|edit| match edit {
            Mutation::SetAttribute {
                name: "id", value: AttributeValue::Text(value), id, ..
            } if matches_id(value) => Some((*id, value.as_str())),
            _ => None,
        })
        .unwrap()
}

fn dispatch(dom: &VirtualDom, target: ElementId, event: &str, key: Option<Key>) {
    let bubbles = key.is_some();
    let data = PlatformEventData::new(Box::new(TestInput(key.unwrap_or(Key::Unidentified))));
    dom.runtime().handle_event(event, Event::new(Rc::new(data), bubbles).into_any(), target);
}

#[derive(Clone)]
struct TestInput(Key);

impl InteractionLocation for TestInput {
    fn client_coordinates(&self) -> ClientPoint {
        ClientPoint::zero()
    }
    fn screen_coordinates(&self) -> ScreenPoint {
        ScreenPoint::zero()
    }
    fn page_coordinates(&self) -> PagePoint {
        PagePoint::zero()
    }
}

impl InteractionElementOffset for TestInput {
    fn element_coordinates(&self) -> ElementPoint {
        ElementPoint::zero()
    }
}

impl ModifiersInteraction for TestInput {
    fn modifiers(&self) -> Modifiers {
        Modifiers::empty()
    }
}

impl PointerInteraction for TestInput {
    fn trigger_button(&self) -> Option<MouseButton> {
        None
    }
    fn held_buttons(&self) -> MouseButtonSet {
        MouseButtonSet::empty()
    }
}

impl HasMouseData for TestInput {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl HasKeyboardData for TestInput {
    fn key(&self) -> Key {
        self.0.clone()
    }
    fn code(&self) -> Code {
        Code::Unidentified
    }
    fn location(&self) -> Location {
        Location::Standard
    }
    fn is_auto_repeating(&self) -> bool {
        false
    }
    fn is_composing(&self) -> bool {
        false
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct EventConverter;

macro_rules! convert_event {
    (convert_mouse_data, MouseData, $event:ident) => {
        MouseData::new($event.downcast::<TestInput>().unwrap().clone())
    };
    (convert_keyboard_data, KeyboardData, $event:ident) => {
        KeyboardData::new($event.downcast::<TestInput>().unwrap().clone())
    };
    ($method:ident, $data:ident, $event:ident) => {
        panic!("测试没有派发 {} 事件", stringify!($method))
    };
}

macro_rules! impl_event_converter {
    (enum Event { $(
        #[convert = $method:ident] #[events = $events:tt] $(#[raw = $raw:tt])?
        $group:ident($data:ident),
    )* }) => {
        impl HtmlEventConverter for EventConverter {
            $(
                fn $method(&self, _event: &PlatformEventData) -> html::$data {
                    convert_event!($method, $data, _event)
                }
            )*
        }
    };
}

html::with_html_event_groups!(impl_event_converter);

fn run_with_event_converter(test_name: &str, run: impl FnOnce()) {
    // 转换器是进程级状态，避免替换共享组件其它回归的转换器。
    const CHILD: &str = "AUCTION_TRUNCATED_TEXT_EVENT_TEST";
    if std::env::var_os(CHILD).is_none() {
        let (_, path) = module_path!().split_once("::").unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("{path}::{test_name}"), "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    html::set_event_converter(Box::new(EventConverter));
    run();
}

#[test]
fn component_escape_respects_modal_and_restores_focus() {
    run_with_event_converter("component_escape_respects_modal_and_restores_focus", || {
        for (inside_dialog, open_after_hover) in [(false, false), (false, true), (true, false)] {
            let browser = Rc::new(Browser { inside_dialog, ..Browser::default() });
            let mut dom = VirtualDom::new(scene).with_root_context(Rc::clone(&browser));
            let initial = dom.rebuild_to_vec().edits;
            let (anchor, trigger_id) = element(&initial, |id| id.ends_with("-trigger"));
            if inside_dialog {
                browser.add_element(DIALOG_ID, ElementKind::AriaDialog, None);
            }
            browser.add_control(trigger_id, inside_dialog);
            browser.add_control(OUTSIDE_ID, false);
            settle(&mut dom);
            assert!(browser.keydown.borrow().is_empty());

            dispatch(&dom, anchor, "mouseenter", None);
            let opened = settle(&mut dom);
            let (_, popup_id) =
                element(&opened, |id| id == trigger_id.trim_end_matches("-trigger"));
            browser.add_control(popup_id, inside_dialog);
            assert_eq!(browser.keydown.borrow().len(), 1);
            assert!(opened.iter().any(|edit| matches!(edit,
                Mutation::CreateTextNode { value, .. } if value == "Full equipment name"
            )));

            if open_after_hover {
                // 键盘固定的背景提示在抽屉打开后仍存在。
                dispatch(&dom, anchor, "keydown", Some(Key::Enter));
                browser.open_dialog.get().unwrap().set(true);
                let dialog_edits = settle(&mut dom);
                let (dialog, _) = element(&dialog_edits, |id| id == DIALOG_ID);
                browser.add_element(DIALOG_ID, ElementKind::AriaDialog, None);
                browser.focused.replace(DIALOG_ID.to_owned());
                assert!(!browser.dispatch_escape());
                dispatch(&dom, dialog, "keydown", Some(Key::Escape));
                settle(&mut dom);
                assert!(!browser.open_dialog.get().unwrap()());
                assert_eq!(browser.keydown.borrow().len(), 1);
                browser.nodes.borrow_mut().remove(DIALOG_ID);
            }

            browser.focused.replace(if inside_dialog { popup_id } else { OUTSIDE_ID }.to_owned());
            assert!(browser.dispatch_escape());
            let closed = settle(&mut dom);
            assert!(closed.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { name: "aria-expanded", value: AttributeValue::Text(value), id, .. }
                if *id == anchor && value == "false"
        )));
            assert_eq!(
                *browser.focused.borrow(),
                if inside_dialog { trigger_id } else { OUTSIDE_ID }
            );
            assert!(!browser.dispatch_escape());
            assert!(browser.keydown.borrow().is_empty());

            dispatch(&dom, anchor, "mouseenter", None);
            settle(&mut dom);
            assert_eq!(browser.keydown.borrow().len(), 1);
            drop(dom);
            assert!(browser.keydown.borrow().is_empty());
        }
    });
}

#[test]
fn background_tooltip_leaves_escape_to_a_native_modal() {
    run_with_event_converter("background_tooltip_leaves_escape_to_a_native_modal", || {
        for (modal_selector_unsupported, modal) in [(false, true), (true, true), (false, false)] {
            let browser = Rc::new(Browser {
                native_dialog: true,
                modal_selector_unsupported,
                ..Browser::default()
            });
            let mut dom = VirtualDom::new(scene).with_root_context(Rc::clone(&browser));
            let initial = dom.rebuild_to_vec().edits;
            let (anchor, trigger_id) = element(&initial, |id| id.ends_with("-trigger"));
            browser.add_control(trigger_id, false);
            settle(&mut dom);
            dispatch(&dom, anchor, "mouseenter", None);
            let opened = settle(&mut dom);
            let (_, popup_id) =
                element(&opened, |id| id == trigger_id.trim_end_matches("-trigger"));
            browser.add_control(popup_id, false);
            dispatch(&dom, anchor, "keydown", Some(Key::Enter));

            browser.open_dialog.get().unwrap().set(true);
            let dialog_edits = settle(&mut dom);
            let (_, dialog_id) = element(&dialog_edits, |id| id == DIALOG_ID);
            browser.add_element(dialog_id, ElementKind::NativeDialog { open: true, modal }, None);
            browser.focused.replace(dialog_id.to_owned());

            // 返回 false 时，浏览器仍可以执行对话框的默认关闭请求。
            assert_eq!(browser.dispatch_escape(), !modal);
            settle(&mut dom);
            assert_eq!(browser.keydown.borrow().len(), usize::from(modal));
        }
    });
}

#[test]
fn equipment_portal_escape_restores_the_outer_link() {
    run_with_event_converter("equipment_portal_escape_restores_the_outer_link", || {
        for (inside_dialog, modal_selector_unsupported) in
            [(false, false), (true, false), (true, true)]
        {
            let browser = Rc::new(Browser {
                inside_dialog,
                modal_selector_unsupported,
                ..Browser::default()
            });
            let mut dom = VirtualDom::new(equipment_link).with_root_context(Rc::clone(&browser));
            let initial = dom.rebuild_to_vec().edits;
            let (anchor, trigger_id) = element(&initial, |id| id.ends_with("-trigger"));
            let (_, link_id) = element(&initial, |id| id == LINK_ID);
            browser.add_element(BODY_ID, ElementKind::Plain, None);
            let parent_id = if inside_dialog {
                let (_, modal_id) = element(&initial, |id| id == DIALOG_ID);
                browser.add_element(
                    modal_id,
                    ElementKind::NativeDialog { open: true, modal: true },
                    browser.body(),
                );
                modal_id
            } else {
                BODY_ID
            };
            browser.add_element(link_id, ElementKind::Link, browser.get_element_by_id(parent_id));
            browser.add_element(trigger_id, ElementKind::Plain, browser.get_element_by_id(link_id));
            settle(&mut dom);

            dispatch(&dom, anchor, "mouseenter", None);
            let opened = settle(&mut dom);
            let popup_id = trigger_id.trim_end_matches("-trigger");
            assert!(opened.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { name: "aria-describedby", value: AttributeValue::Text(value), id, .. }
                if *id == anchor && value == popup_id
        )));
            let popup = browser.get_element_by_id(popup_id).expect("生产代码挂载了可聚焦的 portal");
            assert_eq!(popup.parent.as_ref().unwrap().id, parent_id);
            browser.focus(&popup);
            assert_eq!(*browser.focused.borrow(), popup_id);
            assert!(browser.dispatch_escape());
            settle(&mut dom);
            assert_eq!(*browser.focused.borrow(), LINK_ID);
            assert!(browser.get_element_by_id(popup_id).is_none());
            assert!(browser.keydown.borrow().is_empty());
        }
    });
}
