//! 在宿主虚拟 DOM 中挂载截断提示，替换浏览器的测量、焦点与原生文档事件。

use std::any::Any;
use std::future::Future;
use std::pin::pin;
use std::process::Command;
use std::rc::Rc;
use std::task::{Context, Waker};

use dioxus::core::{AttributeValue, ElementId, Mutation};
use dioxus::html::geometry::{ClientPoint, ElementPoint, PagePoint, ScreenPoint};
use dioxus::html::input_data::{MouseButton, MouseButtonSet};
use dioxus::html::{
    self, Code, HasKeyboardData, HasMouseData, HtmlEventConverter, InteractionElementOffset,
    InteractionLocation, Location, Modifiers, ModifiersInteraction, PlatformEventData,
    PointerInteraction,
};
use dioxus::prelude::*;

use super::host_document::{BODY_ID, ElementKind, TooltipBrowserFixture as Browser};
use super::{TruncatedText, use_truncated_text_anchor_order};

const OUTSIDE_ID: &str = "outside";
const DIALOG_ID: &str = "tooltip-test-dialog";
const LINK_ID: &str = "equipment-link";

fn add_control(browser: &Browser, id: &str, inside_dialog: bool) {
    let parent = inside_dialog.then(|| browser.get_element_by_id(DIALOG_ID).unwrap());
    browser.add_element(id, ElementKind::Control, parent);
}

fn scene() -> Element {
    use_truncated_text_anchor_order(3);
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
    use_truncated_text_anchor_order(3);
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
            add_control(&browser, trigger_id, inside_dialog);
            add_control(&browser, OUTSIDE_ID, false);
            settle(&mut dom);
            assert!(browser.keydown.borrow().is_empty());

            dispatch(&dom, anchor, "mouseenter", None);
            let opened = settle(&mut dom);
            let (_, popup_id) =
                element(&opened, |id| id == trigger_id.trim_end_matches("-trigger"));
            add_control(&browser, popup_id, inside_dialog);
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
            assert!(browser.frames.borrow().is_empty());

            dispatch(&dom, anchor, "mouseenter", None);
            settle(&mut dom);
            assert_eq!(browser.keydown.borrow().len(), 1);
            assert_eq!(browser.frames.borrow().len(), 1);
            drop(dom);
            assert!(browser.keydown.borrow().is_empty());
            assert!(browser.frames.borrow().is_empty());
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
            add_control(&browser, trigger_id, false);
            settle(&mut dom);
            dispatch(&dom, anchor, "mouseenter", None);
            let opened = settle(&mut dom);
            let (_, popup_id) =
                element(&opened, |id| id == trigger_id.trim_end_matches("-trigger"));
            add_control(&browser, popup_id, false);
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

fn nested_dialog_scene() -> Element {
    let browser = use_context::<Rc<Browser>>();
    let mut inner_open = use_signal(|| false);
    browser.open_dialog.set(Some(inner_open));
    let inner_id = "inner-dialog";
    rsx! {
        dialog { id: DIALOG_ID,
            TruncatedText { value: "Outer equipment name", portal: true }
            if inner_open() {
                div { id: "{inner_id}", role: "dialog", aria_modal: "true",
                    onkeydown: move |event| {
                        if event.key() == Key::Escape {
                            event.prevent_default();
                            inner_open.set(false);
                        }
                    },
                    TruncatedText { value: "Inner equipment name", portal: true }
                }
            }
        }
    }
}

#[test]
fn a_native_modal_limits_escape_to_its_foreground_application_dialog() {
    run_with_event_converter(
        "a_native_modal_limits_escape_to_its_foreground_application_dialog",
        || {
            for modal_selector_unsupported in [false, true] {
                let browser = Rc::new(Browser { modal_selector_unsupported, ..Browser::default() });
                let mut dom =
                    VirtualDom::new(nested_dialog_scene).with_root_context(browser.clone());
                let initial = dom.rebuild_to_vec().edits;
                let (outer_anchor, outer_trigger_id) =
                    element(&initial, |id| id.ends_with("-trigger"));
                browser.add_element(
                    DIALOG_ID,
                    ElementKind::NativeDialog { open: true, modal: true },
                    None,
                );
                add_control(&browser, outer_trigger_id, true);
                settle(&mut dom);
                dispatch(&dom, outer_anchor, "mouseenter", None);
                settle(&mut dom);
                let outer_popup_id = outer_trigger_id.trim_end_matches("-trigger");
                assert!(browser.get_element_by_id(outer_popup_id).is_some());

                let mut inner_open = browser.open_dialog.get().unwrap();
                inner_open.set(true);
                let dialog_edits = settle(&mut dom);
                let (inner_dialog, inner_id) = element(&dialog_edits, |id| id == "inner-dialog");
                let (inner_anchor, inner_trigger_id) =
                    element(&dialog_edits, |id| id.ends_with("-trigger"));
                browser.add_element(
                    inner_id,
                    ElementKind::AriaDialog,
                    browser.get_element_by_id(DIALOG_ID),
                );
                browser.add_element(
                    inner_trigger_id,
                    ElementKind::Control,
                    browser.get_element_by_id(inner_id),
                );
                browser.focused.replace(inner_trigger_id.to_owned());
                assert!(!browser.dispatch_escape(), "外层提示把 Escape 留给前景模态");

                dispatch(&dom, inner_anchor, "mouseenter", None);
                settle(&mut dom);
                let inner_popup_id = inner_trigger_id.trim_end_matches("-trigger");
                let inner_popup = browser.get_element_by_id(inner_popup_id).unwrap();
                assert_eq!(inner_popup.parent.as_ref().unwrap().id, inner_id);
                browser.focus(&inner_popup);
                assert!(browser.dispatch_escape(), "前景模态自己的提示处理 Escape");
                settle(&mut dom);
                assert!(inner_open());
                assert_eq!(*browser.focused.borrow(), inner_trigger_id);
                assert!(browser.get_element_by_id(outer_popup_id).is_some());
                assert!(!browser.dispatch_escape());
                dispatch(&dom, inner_dialog, "keydown", Some(Key::Escape));
                settle(&mut dom);
                assert!(!inner_open());
            }
        },
    );
}

#[test]
fn open_tooltip_follows_the_anchor_and_pauses_when_hidden() {
    run_with_event_converter("open_tooltip_follows_the_anchor_and_pauses_when_hidden", || {
        let browser = Rc::new(Browser::new());
        let mut dom = VirtualDom::new(equipment_link).with_root_context(browser.clone());
        let initial = dom.rebuild_to_vec().edits;
        let (anchor, trigger_id) = element(&initial, |id| id.ends_with("-trigger"));
        browser.add_element(LINK_ID, ElementKind::Link, browser.body());
        browser.add_element(trigger_id, ElementKind::Plain, browser.get_element_by_id(LINK_ID));
        let trigger = browser.get_element_by_id(trigger_id).unwrap();
        settle(&mut dom);
        assert!(browser.frames.borrow().is_empty());

        dispatch(&dom, anchor, "mouseenter", None);
        settle(&mut dom);
        let popup_id = trigger_id.trim_end_matches("-trigger");
        let popup = browser.get_element_by_id(popup_id).unwrap();
        browser.focus(&popup);
        let portal = popup.portal_writes.unwrap();
        let top = || portal.values.borrow()["--truncated-text-tooltip-top"].clone();
        assert_eq!(top(), "88px");
        let writes = portal.count.get();
        assert_eq!(browser.frames.borrow().len(), 1);
        let reads = trigger.rectangle_reads.get();
        for rectangle in [(20.0, 50.0, 120.0, 80.0), (20.1, 50.1, 120.1, 80.1)] {
            trigger.rectangle.set(rectangle);
            browser.advance_frame();
            assert!(
                pin!(dom.wait_for_work())
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        assert_eq!(trigger.rectangle_reads.get(), reads + 2);
        assert_eq!(browser.frames.borrow().len(), 1);
        assert_eq!(portal.count.get(), writes);

        trigger.rectangle.set((20.0, 110.0, 120.0, 140.0));
        browser.advance_frame();
        assert_eq!(top(), "148px");
        assert!(Rc::ptr_eq(
            &portal,
            &browser.get_element_by_id(popup_id).unwrap().portal_writes.unwrap(),
        ));
        assert_eq!(browser.focus_id(), popup_id);
        settle(&mut dom);

        browser.set_hidden(true);
        assert!(browser.frames.borrow().is_empty());
        trigger.rectangle.set((20.0, 150.0, 120.0, 180.0));
        let reads = trigger.rectangle_reads.get();
        browser.advance_frame();
        assert_eq!(trigger.rectangle_reads.get(), reads);
        browser.set_hidden(false);
        assert_eq!(browser.frames.borrow().len(), 1);
        browser.advance_frame();
        assert_eq!(top(), "188px");
        assert_eq!(browser.focus_id(), popup_id);
        settle(&mut dom);

        browser.set_hidden(true);
        assert!(!browser.visibility.borrow().is_empty());
        drop(dom);
        assert!(browser.visibility.borrow().is_empty());
        browser.set_hidden(false);
        assert!(browser.frames.borrow().is_empty());
    });
}

#[test]
fn subpixel_jitter_across_the_viewport_midline_leaves_the_portal_untouched() {
    run_with_event_converter(
        "subpixel_jitter_across_the_viewport_midline_leaves_the_portal_untouched",
        || {
            let browser = Rc::new(Browser::new());
            let mut dom = VirtualDom::new(equipment_link).with_root_context(browser.clone());
            let initial = dom.rebuild_to_vec().edits;
            let (anchor, trigger_id) = element(&initial, |id| id.ends_with("-trigger"));
            browser.add_focusable(trigger_id);
            let trigger = browser.get_element_by_id(trigger_id).unwrap();
            trigger.rectangle.set((20.0, 363.49, 120.0, 404.49));
            settle(&mut dom);
            dispatch(&dom, anchor, "mouseenter", None);
            settle(&mut dom);
            let popup_id = trigger_id.trim_end_matches("-trigger");
            let portal = browser.get_element_by_id(popup_id).unwrap().portal_writes.unwrap();
            let writes = portal.count.get();
            for top in [363.51, 363.49, 363.51, 363.79, 363.98] {
                trigger.rectangle.set((20.0, top, 120.0, top + 41.0));
                browser.advance_frame();
                let values = portal.values.borrow();
                assert_eq!(values["--truncated-text-tooltip-top"], "412.49px");
                assert_eq!(values["data-placement"], "below");
                assert_eq!(portal.count.get(), writes);
                assert!(
                    pin!(dom.wait_for_work())
                        .poll(&mut Context::from_waker(Waker::noop()))
                        .is_pending()
                );
            }
            trigger.rectangle.set((20.0, 364.0, 120.0, 405.0));
            browser.advance_frame();
            let values = portal.values.borrow();
            assert_eq!(values["--truncated-text-tooltip-top"], "356px");
            assert_eq!(values["data-placement"], "above");
        },
    );
}

#[test]
fn an_ancestor_clip_closes_a_portal_within_the_viewport() {
    run_with_event_converter("an_ancestor_clip_closes_a_portal_within_the_viewport", || {
        let browser = Rc::new(Browser::new());
        let mut dom = VirtualDom::new(equipment_link).with_root_context(browser.clone());
        let initial = dom.rebuild_to_vec().edits;
        let (anchor, trigger_id) = element(&initial, |id| id.ends_with("-trigger"));
        browser.add_element("clip", ElementKind::Plain, browser.body());
        let clip = browser.get_element_by_id("clip").unwrap();
        clip.overflow.set(("clip", "visible"));
        clip.rectangle.set((0.0, 150.0, 100.0, 200.0));
        browser.add_element(LINK_ID, ElementKind::Link, Some(clip.clone()));
        browser.add_element(trigger_id, ElementKind::Plain, browser.get_element_by_id(LINK_ID));
        let trigger = browser.get_element_by_id(trigger_id).unwrap();
        trigger.rectangle.set((1.0, 50.0, 51.0, 80.0));
        settle(&mut dom);
        dispatch(&dom, anchor, "mouseenter", None);
        settle(&mut dom);
        let popup_id = trigger_id.trim_end_matches("-trigger");
        browser.focus_by_id(popup_id);

        browser.advance_frame();
        assert_eq!(clip.overflow_reads.get(), 0);
        trigger.rectangle.set((2.0, 50.0, 52.0, 80.0));
        browser.advance_frame();
        assert!(browser.get_element_by_id(popup_id).is_some());
        assert_eq!(browser.focus_id(), popup_id);
        let reads_after_move = clip.overflow_reads.get();
        browser.advance_frame();
        assert_eq!(clip.overflow_reads.get(), reads_after_move);

        trigger.rectangle.set((100.0, 50.0, 150.0, 80.0));
        browser.advance_frame();
        assert!(browser.get_element_by_id(popup_id).is_none());
        assert_eq!(reads_after_move, 1, "定位被视口边距夹住时也只遍历一次");
        assert_eq!(browser.focus_id(), LINK_ID);
        assert!(browser.frames.borrow().is_empty());
    });
}

#[test]
fn lost_anchor_closes_the_portal_and_only_restores_its_own_focus() {
    run_with_event_converter(
        "lost_anchor_closes_the_portal_and_only_restores_its_own_focus",
        || {
            for (rectangle, popup_focused, link_connected) in [
                (None, true, true),
                (None, true, false),
                (None, false, true),
                (Some((20.0, 50.0, 20.0, 80.0)), true, true),
                (Some((20.0, 768.0, 120.0, 798.0)), false, true),
            ] {
                let browser = Rc::new(Browser::new());
                let mut dom = VirtualDom::new(equipment_link).with_root_context(browser.clone());
                let initial = dom.rebuild_to_vec().edits;
                let (anchor, trigger_id) = element(&initial, |id| id.ends_with("-trigger"));
                browser.add_element(LINK_ID, ElementKind::Link, browser.body());
                browser.add_element(
                    trigger_id,
                    ElementKind::Plain,
                    browser.get_element_by_id(LINK_ID),
                );
                browser.add_focusable(OUTSIDE_ID);
                settle(&mut dom);
                dispatch(&dom, anchor, "mouseenter", None);
                settle(&mut dom);
                let popup_id = trigger_id.trim_end_matches("-trigger");
                browser.focus_by_id(if popup_focused { popup_id } else { OUTSIDE_ID });
                assert_eq!(browser.frames.borrow().len(), 1);

                if let Some(rectangle) = rectangle {
                    browser.get_element_by_id(trigger_id).unwrap().rectangle.set(rectangle);
                } else {
                    browser.nodes.borrow_mut().remove(trigger_id);
                }
                if !link_connected {
                    browser.nodes.borrow_mut().remove(LINK_ID);
                }
                browser.advance_frame();
                assert!(browser.get_element_by_id(popup_id).is_none());
                let expected_focus = match (popup_focused, link_connected) {
                    (true, true) => LINK_ID,
                    (true, false) => BODY_ID,
                    (false, _) => OUTSIDE_ID,
                };
                assert_eq!(browser.focus_id(), expected_focus);
                assert!(browser.frames.borrow().is_empty());
                assert!(browser.visibility.borrow().is_empty());
            }
        },
    );
}
