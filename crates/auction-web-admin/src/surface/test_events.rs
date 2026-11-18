//! 宿主测试共用的 Dioxus 表单与焦点事件适配。

use std::any::Any;
use std::rc::Rc;
use std::sync::Once;

use dioxus::core::{ElementId, Event, VirtualDom};
use dioxus::html::{
    FileData, FocusData, FormData, FormValue, HasFileData, HasFocusData, HasFormData,
    HtmlEventConverter, PlatformEventData, set_event_converter, with_html_event_groups,
};

pub(super) fn dispatch_form_event(
    dom: &VirtualDom,
    target: ElementId,
    name: &str,
    value: &str,
) -> Event<dyn Any> {
    // Dioxus 的转换器是进程级状态。并行的表单测试共用一次注册和同一种事件数据。
    static CONVERTER: Once = Once::new();
    CONVERTER.call_once(|| set_event_converter(Box::new(FormEventConverter)));
    let data = PlatformEventData::new(Box::new(FormValueEvent(value.to_owned())));
    let event: Event<dyn Any> = Event::new(Rc::new(data), !matches!(name, "blur" | "focus"));
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

struct FormEventConverter;

macro_rules! form_event_conversion {
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
    ($group:ident, $converter:ident, $data:ident) => {
        fn $converter(&self, _event: &PlatformEventData) -> dioxus::html::$data {
            panic!("测试没有派发 {} 事件", stringify!($group));
        }
    };
}

macro_rules! impl_form_event_converter {
    (enum Event { $(
        #[convert = $converter:ident] #[events = $events:tt] $(#[raw = $raw:tt])?
        $group:ident($data:ident),
    )* }) => {
        impl HtmlEventConverter for FormEventConverter {
            $(form_event_conversion!($group, $converter, $data);)*
        }
    };
}

with_html_event_groups!(impl_form_event_converter);
