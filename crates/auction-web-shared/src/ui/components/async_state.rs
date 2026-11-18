//! 加载 / 错误 / 空结果三态面板。
//! 差异只在无障碍角色与是否提供重试入口；文案一律由调用方经 props 传入。
//! 回调属性须为非可选类型，避免更新时拷入会随临时 VNode 释放的句柄
//! （Dioxus 0.7.10 的 `dioxus-core-macro/src/props/mod.rs:694–715` 与 `dioxus-core/src/events.rs:555–556`）。

use std::sync::atomic::{AtomicUsize, Ordering};

use dioxus::prelude::*;

/// 同一页面可能出现多个状态面板，`aria-labelledby` 需要全页唯一的 id。
static NEXT_ASYNC_STATE_ID: AtomicUsize = AtomicUsize::new(1);

/// 异步状态面板的三种语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsyncStateKind {
    /// 正在加载：`role="status"` + `aria-live="polite"`，屏幕阅读器礼貌播报。
    Loading,
    /// 加载失败：`role="alert"`（自带 assertive 播报语义），可附带重试按钮。
    Error,
    /// 成功但无数据：`role="status"` + `aria-live="polite"`。
    Empty,
}

impl AsyncStateKind {
    const fn modifier(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Error => "error",
            Self::Empty => "empty",
        }
    }

    const fn role(self) -> &'static str {
        match self {
            Self::Error => "alert",
            Self::Loading | Self::Empty => "status",
        }
    }
}

/// 展示异步数据的加载、错误或空结果状态。
/// 只呈现调用方已判定的安全文案，不把错误正文直接展示给浏览器。提供 `retry_label` 时渲染重试按钮。
#[component]
pub fn AsyncState(
    kind: AsyncStateKind,
    title: String,
    detail: String,
    /// 重试按钮文案。
    #[props(default)]
    retry_label: Option<String>,
    /// 重试只调用当前属性槽保存的回调，缺省回调为空操作。
    #[props(default)]
    on_retry: EventHandler<()>,
) -> Element {
    let title_id = use_hook(|| {
        format!("async-state-title-{}", NEXT_ASYNC_STATE_ID.fetch_add(1, Ordering::Relaxed))
    });
    let class = format!("ui-async-state ui-async-state--{}", kind.modifier());
    // role="alert" 已隐含 assertive 的 aria-live，无需重复声明，避免部分阅读器双报。
    let aria_live = if kind == AsyncStateKind::Error { None } else { Some("polite") };

    rsx! {
        section {
            class: "{class}",
            role: kind.role(),
            aria_live,
            aria_labelledby: "{title_id}",
            h2 { id: "{title_id}", class: "ui-async-state__title", "{title}" }
            p { class: "ui-async-state__detail", "{detail}" }
            if let Some(label) = retry_label {
                button {
                    class: "ui-async-state__retry",
                    r#type: "button",
                    onclick: move |_| on_retry.call(()),
                    "{label}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{any::Any, cell::Cell, rc::Rc};

    use dioxus::dioxus_core::{ElementId, Mutation, Mutations};
    use dioxus::html::geometry::{ClientPoint, ElementPoint, PagePoint, ScreenPoint};
    use dioxus::html::input_data::{MouseButton, MouseButtonSet};
    use dioxus::html::{
        InteractionElementOffset, InteractionLocation, Modifiers, ModifiersInteraction,
        PointerInteraction,
    };
    use dioxus::prelude::*;

    use super::{AsyncState, AsyncStateKind};

    struct TestMouse;

    impl InteractionLocation for TestMouse {
        fn client_coordinates(&self) -> ClientPoint {
            ClientPoint::origin()
        }

        fn page_coordinates(&self) -> PagePoint {
            PagePoint::origin()
        }

        fn screen_coordinates(&self) -> ScreenPoint {
            ScreenPoint::origin()
        }
    }

    impl InteractionElementOffset for TestMouse {
        fn element_coordinates(&self) -> ElementPoint {
            ElementPoint::origin()
        }
    }

    impl ModifiersInteraction for TestMouse {
        fn modifiers(&self) -> Modifiers {
            Modifiers::empty()
        }
    }

    impl PointerInteraction for TestMouse {
        fn trigger_button(&self) -> Option<MouseButton> {
            Some(MouseButton::Primary)
        }

        fn held_buttons(&self) -> MouseButtonSet {
            MouseButtonSet::empty()
        }
    }

    impl HasMouseData for TestMouse {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct TestEventConverter;

    macro_rules! convert_event {
        (convert_mouse_data, MouseData) => {
            MouseData::new(TestMouse)
        };
        ($method:ident, $data:ident) => {
            panic!("意外的事件转换：{}", stringify!($method))
        };
    }

    macro_rules! impl_test_event_converter {
        (enum Event {
            $(
                #[convert = $method:ident]
                #[events = $events:tt]
                $(#[raw = $raw:tt])?
                $group:ident($data:ident),
            )*
        }) => {
            impl HtmlEventConverter for TestEventConverter {
                $(
                    fn $method(&self, _: &PlatformEventData) -> $data {
                        convert_event!($method, $data)
                    }
                )*
            }
        };
    }

    dioxus::html::with_html_event_groups!(impl_test_event_converter);

    #[derive(Default)]
    struct StateFixture {
        revision: Cell<u8>,
        called: Cell<Option<u8>>,
    }

    fn async_state_fixture() -> Element {
        let fixture = consume_context::<Rc<StateFixture>>();
        let revision = fixture.revision.get();
        if revision == 0 {
            rsx! {
                AsyncState { kind: AsyncStateKind::Loading, title: "加载中", detail: "" }
            }
        } else {
            rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error, title: "读取失败", detail: "{revision}",
                    retry_label: "重试",
                    on_retry: move |()| fixture.called.set(Some(revision)),
                }
            }
        }
    }

    fn retry_button(mutations: &Mutations) -> Option<ElementId> {
        mutations.edits.iter().find_map(|edit| match edit {
            Mutation::NewEventListener { name, id } if name == "click" => Some(*id),
            _ => None,
        })
    }

    fn click(dom: &VirtualDom, button: ElementId) {
        let event = Event::new(Rc::new(PlatformEventData::new(Box::new(()))) as Rc<dyn Any>, true);
        dom.runtime().handle_event("click", event, button);
    }

    #[test]
    fn retry_uses_the_latest_callback_after_loading_turns_into_error() {
        set_event_converter(Box::new(TestEventConverter));
        let fixture = Rc::new(StateFixture::default());
        let mut dom = VirtualDom::new(async_state_fixture);
        dom.provide_root_context(fixture.clone());
        assert_eq!(retry_button(&dom.rebuild_to_vec()), None);

        fixture.revision.set(1);
        dom.mark_dirty(ScopeId::APP);
        let button = retry_button(&dom.render_immediate_to_vec()).expect("错误态有重试按钮");
        for revision in [2, 3] {
            fixture.revision.set(revision);
            dom.mark_dirty(ScopeId::APP);
            assert_eq!(retry_button(&dom.render_immediate_to_vec()), None);
        }
        assert_eq!(fixture.called.get(), None);
        click(&dom, button);
        assert_eq!(fixture.called.get(), Some(3));
    }

    #[test]
    fn retry_button_visibility_depends_on_the_label() {
        set_event_converter(Box::new(TestEventConverter));
        let mut label_only = VirtualDom::new(|| {
            rsx! {
                AsyncState { kind: AsyncStateKind::Error, title: "读取失败", detail: "", retry_label: "重试" }
            }
        });
        let button = retry_button(&label_only.rebuild_to_vec()).expect("文案启用重试按钮");
        click(&label_only, button);

        let mut callback_only = VirtualDom::new(|| {
            rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error, title: "读取失败", detail: "",
                    on_retry: move |()| {},
                }
            }
        });
        assert_eq!(retry_button(&callback_only.rebuild_to_vec()), None);
    }
}
