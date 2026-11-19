//! 读取页宿主回归的原生选择器事件和选项检查。

use std::any::Any;
use std::rc::Rc;

use dioxus::dioxus_core::{
    AttributeValue, DynamicNode, ElementId, TemplateAttribute, TemplateNode,
};
use dioxus::html::{
    self, FileData, FocusData, FormData, FormValue, HasFileData, HasFocusData, HasFormData,
    HtmlEventConverter, PlatformEventData,
};
use dioxus::prelude::*;

pub(in crate::surface) fn select_values(
    dom: &VirtualDom,
    attribute: &str,
    value: &str,
) -> Vec<String> {
    let root = dom.base_scope().root_node();
    let mut values = None;
    for node in root.template.roots {
        visit(dom, root, node, &mut |vnode, node| {
            let TemplateNode::Element { tag: "select", attrs, .. } = node else { return true };
            if text_attribute(vnode, attrs, attribute) != Some(value) {
                return true;
            }
            let mut options = Vec::new();
            visit(dom, vnode, node, &mut |vnode, node| {
                if let TemplateNode::Element { tag: "option", attrs, .. } = node {
                    options.push(text_attribute(vnode, attrs, "value").unwrap().to_owned());
                }
                true
            });
            assert!(values.replace(options).is_none());
            false
        });
    }
    values.expect("当前页面的世界或场次选择器已挂载")
}

fn visit(
    dom: &VirtualDom,
    vnode: &VNode,
    node: &TemplateNode,
    visitor: &mut impl FnMut(&VNode, &TemplateNode) -> bool,
) {
    if !visitor(vnode, node) {
        return;
    }
    match node {
        TemplateNode::Element { children, .. } => {
            for child in *children {
                visit(dom, vnode, child, visitor);
            }
        }
        TemplateNode::Dynamic { id } => match &vnode.dynamic_nodes[*id] {
            DynamicNode::Component(component) => {
                if let Some(root) =
                    component.mounted_scope(*id, vnode, dom).and_then(|scope| scope.try_root_node())
                {
                    for child in root.template.roots {
                        visit(dom, root, child, visitor);
                    }
                }
            }
            DynamicNode::Fragment(nodes) => {
                for vnode in nodes {
                    for child in vnode.template.roots {
                        visit(dom, vnode, child, visitor);
                    }
                }
            }
            _ => {}
        },
        TemplateNode::Text { .. } => {}
    }
}

fn text_attribute<'a>(
    vnode: &'a VNode,
    attrs: &[TemplateAttribute],
    name: &str,
) -> Option<&'a str> {
    attrs.iter().find_map(|attr| match attr {
        TemplateAttribute::Static { name: attribute, value, .. } if *attribute == name => {
            Some(*value)
        }
        TemplateAttribute::Dynamic { id } => {
            vnode.dynamic_attrs[*id].iter().find_map(|attr| match &attr.value {
                AttributeValue::Text(text) if attr.name == name => Some(text.as_str()),
                _ => None,
            })
        }
        _ => None,
    })
}

// 事件转换器是进程全局的，只在 run_page_events 的隔离进程中使用。
pub(super) fn dispatch_select_event(dom: &VirtualDom, select: ElementId, value: Option<&str>) {
    html::set_event_converter(Box::new(SelectEventConverter));
    let (name, bubbles) = if value.is_some() { ("change", true) } else { ("focus", false) };
    let payload = SelectEvent(value.unwrap_or_default().to_owned());
    let event = Event::new(Rc::new(PlatformEventData::new(Box::new(payload))), bubbles);
    dom.runtime().handle_event(name, event.into_any(), select);
}

struct SelectEventConverter;

macro_rules! unsupported {
    ($($convert:ident -> $data:ident),* $(,)?) => {
        $(fn $convert(&self, _event: &PlatformEventData) -> html::$data {
            panic!("该测试替身只处理选择器的 focus 和 change 事件");
        })*
    };
}

impl HtmlEventConverter for SelectEventConverter {
    fn convert_focus_data(&self, event: &PlatformEventData) -> FocusData {
        FocusData::new(event.downcast::<SelectEvent>().unwrap().clone())
    }

    fn convert_form_data(&self, event: &PlatformEventData) -> FormData {
        FormData::new(event.downcast::<SelectEvent>().unwrap().clone())
    }

    unsupported!(
        convert_animation_data -> AnimationData, convert_cancel_data -> CancelData,
        convert_clipboard_data -> ClipboardData, convert_composition_data -> CompositionData,
        convert_drag_data -> DragData, convert_image_data -> ImageData,
        convert_keyboard_data -> KeyboardData, convert_media_data -> MediaData,
        convert_mounted_data -> MountedData, convert_mouse_data -> MouseData,
        convert_pointer_data -> PointerData, convert_resize_data -> ResizeData,
        convert_scroll_data -> ScrollData, convert_selection_data -> SelectionData,
        convert_touch_data -> TouchData, convert_toggle_data -> ToggleData,
        convert_transition_data -> TransitionData, convert_visible_data -> VisibleData,
        convert_wheel_data -> WheelData,
    );
}

#[derive(Clone)]
struct SelectEvent(String);

impl HasFocusData for SelectEvent {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl HasFileData for SelectEvent {
    fn files(&self) -> Vec<FileData> {
        Vec::new()
    }
}

impl HasFormData for SelectEvent {
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
