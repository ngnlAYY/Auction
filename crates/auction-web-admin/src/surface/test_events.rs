//! 宿主测试共用的 Dioxus 表单、焦点与点击事件适配。

use std::any::Any;
use std::rc::Rc;
use std::sync::Once;

use dioxus::core::{ElementId, Event, VirtualDom};
use dioxus::html::geometry::{ClientPoint, ElementPoint, PagePoint, ScreenPoint};
use dioxus::html::input_data::{MouseButton, MouseButtonSet};
use dioxus::html::{
    FileData, FocusData, FormData, FormValue, HasFileData, HasFocusData, HasFormData, HasMouseData,
    HtmlEventConverter, InteractionElementOffset, InteractionLocation, Modifiers,
    ModifiersInteraction, MouseData, PlatformEventData, PointerInteraction, set_event_converter,
    with_html_event_groups,
};

pub(super) fn dispatch_form_event(
    dom: &VirtualDom,
    target: ElementId,
    name: &str,
    value: &str,
) -> Event<dyn Any> {
    dispatch(
        dom,
        target,
        name,
        Box::new(FormValueEvent(value.to_owned())),
        !matches!(name, "blur" | "focus"),
    )
}

pub(super) fn dispatch_click_event(dom: &VirtualDom, target: ElementId) {
    dispatch(dom, target, "click", Box::new(ClickEvent), true);
}

fn dispatch(
    dom: &VirtualDom,
    target: ElementId,
    name: &str,
    data: Box<dyn Any>,
    bubbles: bool,
) -> Event<dyn Any> {
    // Dioxus 的转换器是进程级状态，并行测试共用一次注册。
    static CONVERTER: Once = Once::new();
    CONVERTER.call_once(|| set_event_converter(Box::new(TestEventConverter)));
    let event: Event<dyn Any> = Event::new(Rc::new(PlatformEventData::new(data)), bubbles);
    dom.runtime().handle_event(name, event.clone(), target);
    event
}

#[derive(Clone)]
struct FormValueEvent(String);

impl HasFileData for FormValueEvent {
    fn files(&self) -> Vec<FileData> {
        Vec::new()
    }
}

impl HasFormData for FormValueEvent {
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

impl HasFocusData for FormValueEvent {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy)]
struct ClickEvent;

impl InteractionLocation for ClickEvent {
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

impl InteractionElementOffset for ClickEvent {
    fn element_coordinates(&self) -> ElementPoint {
        ElementPoint::zero()
    }
}

impl ModifiersInteraction for ClickEvent {
    fn modifiers(&self) -> Modifiers {
        Modifiers::empty()
    }
}

impl PointerInteraction for ClickEvent {
    fn trigger_button(&self) -> Option<MouseButton> {
        Some(MouseButton::Primary)
    }

    fn held_buttons(&self) -> MouseButtonSet {
        MouseButtonSet::empty()
    }
}

impl HasMouseData for ClickEvent {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct TestEventConverter;

macro_rules! event_conversion {
    (Form, $converter:ident, $data:ident) => {
        fn $converter(&self, event: &PlatformEventData) -> FormData {
            FormData::new(event.downcast::<FormValueEvent>().unwrap().clone())
        }
    };
    (Focus, $converter:ident, $data:ident) => {
        fn $converter(&self, event: &PlatformEventData) -> FocusData {
            FocusData::new(event.downcast::<FormValueEvent>().unwrap().clone())
        }
    };
    (Mouse, $converter:ident, $data:ident) => {
        fn $converter(&self, event: &PlatformEventData) -> MouseData {
            MouseData::new(*event.downcast::<ClickEvent>().unwrap())
        }
    };
    ($group:ident, $converter:ident, $data:ident) => {
        fn $converter(&self, _event: &PlatformEventData) -> dioxus::html::$data {
            panic!("测试没有派发 {} 事件", stringify!($group));
        }
    };
}

macro_rules! impl_test_event_converter {
    (enum Event { $(
        #[convert = $converter:ident] #[events = $events:tt] $(#[raw = $raw:tt])?
        $group:ident($data:ident),
    )* }) => {
        impl HtmlEventConverter for TestEventConverter {
            $(event_conversion!($group, $converter, $data);)*
        }
    };
}

with_html_event_groups!(impl_test_event_converter);
