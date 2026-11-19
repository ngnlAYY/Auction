//! 仅在 Markdown 表格的容器可横向滚动时提供键盘入口。

use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::components::resize::{self, MeasureSubscription};

pub(super) fn use_table_scroll() -> (bool, EventHandler<MountedEvent>) {
    let scrollable = use_signal(|| false);
    #[cfg(target_arch = "wasm32")]
    let mut subscriptions = use_signal(Vec::<MeasureSubscription>::new);
    #[cfg(target_arch = "wasm32")]
    use_drop(move || subscriptions.write().clear());
    let mounted = use_callback(move |_event: MountedEvent| {
        #[cfg(target_arch = "wasm32")]
        if let Some(wrapper) = _event.data().downcast::<web_sys::Element>() {
            subscriptions.set(observe(wrapper, scrollable));
        }
    });
    #[cfg(all(test, not(target_arch = "wasm32")))]
    use_effect(move || {
        if let Some(Measurements(sizes)) = try_consume_context() {
            let (scroll_width, client_width, allows_scroll) = sizes();
            update(scrollable, scroll_width, client_width, allows_scroll);
        }
    });
    (scrollable(), mounted)
}

#[cfg(target_arch = "wasm32")]
fn observe(wrapper: &web_sys::Element, scrollable: Signal<bool>) -> Vec<MeasureSubscription> {
    let target = wrapper.clone();
    let measure = move || {
        let client_width = target.client_width();
        let scroll_width = target.scroll_width();
        // display: contents 与只裁剪内容的容器都不应进入 Tab 顺序。
        let allows_scroll = client_width > 0
            && scroll_width > client_width
            && web_sys::window()
                .and_then(|window| window.get_computed_style(&target).ok().flatten())
                .and_then(|style| style.get_property_value("overflow-x").ok())
                .is_some_and(|overflow| matches!(overflow.as_str(), "auto" | "scroll"));
        update(scrollable, scroll_width, client_width, allows_scroll);
    };
    measure();
    // 表格内容变宽时容器可能保持原宽，观察两个尺寸才能识别新出现的溢出。
    [Some(wrapper.clone()), wrapper.first_element_child()]
        .into_iter()
        .flatten()
        .filter_map(|element| resize::observe(&element, measure.clone()))
        .collect()
}

#[cfg(any(target_arch = "wasm32", test))]
fn update(mut scrollable: Signal<bool>, scroll_width: i32, client_width: i32, allows_scroll: bool) {
    let next = allows_scroll && client_width > 0 && scroll_width > client_width;
    if *scrollable.peek() != next {
        scrollable.set(next);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[derive(Clone, Copy)]
pub(super) struct Measurements(pub(super) Signal<(i32, i32, bool)>);
