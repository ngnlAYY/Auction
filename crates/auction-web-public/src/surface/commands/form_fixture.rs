//! 在宿主虚拟 DOM 中分发表单和鼠标事件。

use std::any::Any;
use std::rc::Rc;
use std::sync::Once;

use dioxus::dioxus_core::ElementId;
use dioxus::html::geometry::{ClientPoint, ElementPoint, PagePoint, ScreenPoint};
use dioxus::html::input_data::{MouseButton, MouseButtonSet};
use dioxus::html::{
    self, FileData, FormData, FormValue, HasFileData, HasFormData, HasMouseData,
    HtmlEventConverter, InteractionElementOffset, InteractionLocation, Modifiers,
    ModifiersInteraction, MouseData, PlatformEventData, PointerInteraction,
};
use dioxus::prelude::{Event, VirtualDom};

pub(in crate::surface) fn submit(dom: &VirtualDom, form: ElementId) {
    assert!(dispatch(dom, "submit", form), "实际表单已处理 submit 事件");
}

pub(in crate::surface) fn click_link(dom: &VirtualDom, anchor: ElementId) {
    assert!(dispatch(dom, "click", anchor), "实际链接已处理 click 事件");
}

fn dispatch(dom: &VirtualDom, name: &str, target: ElementId) -> bool {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| html::set_event_converter(Box::new(EventConverter)));
    let event = Event::new(Rc::new(PlatformEventData::new(Box::new(()))), true);
    dom.runtime().handle_event(name, event.clone().into_any(), target);
    !event.default_action_enabled()
}

struct EventConverter;

macro_rules! unsupported {
    ($($convert:ident -> $data:ident),* $(,)?) => {
        $(fn $convert(&self, _event: &PlatformEventData) -> html::$data {
            panic!("事件替身只处理表单与鼠标事件");
        })*
    };
}

impl HtmlEventConverter for EventConverter {
    fn convert_form_data(&self, _event: &PlatformEventData) -> FormData {
        FormData::new(FormPayload)
    }

    fn convert_mouse_data(&self, _event: &PlatformEventData) -> MouseData {
        MouseData::new(PrimaryClick)
    }

    unsupported!(
        convert_animation_data -> AnimationData,
        convert_cancel_data -> CancelData,
        convert_clipboard_data -> ClipboardData,
        convert_composition_data -> CompositionData,
        convert_drag_data -> DragData,
        convert_focus_data -> FocusData,
        convert_image_data -> ImageData,
        convert_keyboard_data -> KeyboardData,
        convert_media_data -> MediaData,
        convert_mounted_data -> MountedData,
        convert_pointer_data -> PointerData,
        convert_resize_data -> ResizeData,
        convert_scroll_data -> ScrollData,
        convert_selection_data -> SelectionData,
        convert_toggle_data -> ToggleData,
        convert_touch_data -> TouchData,
        convert_transition_data -> TransitionData,
        convert_visible_data -> VisibleData,
        convert_wheel_data -> WheelData,
    );
}

struct PrimaryClick;

impl HasMouseData for PrimaryClick {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl InteractionLocation for PrimaryClick {
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

impl InteractionElementOffset for PrimaryClick {
    fn element_coordinates(&self) -> ElementPoint {
        ElementPoint::zero()
    }
}

impl ModifiersInteraction for PrimaryClick {
    fn modifiers(&self) -> Modifiers {
        Modifiers::empty()
    }
}

impl PointerInteraction for PrimaryClick {
    fn trigger_button(&self) -> Option<MouseButton> {
        Some(MouseButton::Primary)
    }

    fn held_buttons(&self) -> MouseButtonSet {
        MouseButtonSet::empty()
    }
}

struct FormPayload;

impl HasFileData for FormPayload {
    fn files(&self) -> Vec<FileData> {
        Vec::new()
    }
}

impl HasFormData for FormPayload {
    fn value(&self) -> String {
        String::new()
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
