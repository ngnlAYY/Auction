//! 将浏览器可视区域投影为 CSS 变量，供软键盘与缩放期间的固定弹窗使用。
//! 没有 VisualViewport 时保留 CSS 的 vh/dvh 回退；不干预浏览器缩放或页面滚动。

/// 每个 surface 的基础样式组件只安装一份监听；卸载时恢复原样式并移除监听。
pub(crate) fn use_visual_viewport() {
    #[cfg(target_arch = "wasm32")]
    {
        use dioxus::prelude::*;
        use std::{cell::RefCell, rc::Rc};

        let guard = use_hook(|| Rc::new(RefCell::new(None::<ViewportGuard>)));
        let cleanup = Rc::clone(&guard);
        use_drop(move || {
            cleanup.borrow_mut().take();
        });
        use_effect(move || {
            if guard.borrow().is_some() {
                return;
            }
            guard.replace(ViewportGuard::install());
        });
    }
}

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

#[cfg(target_arch = "wasm32")]
const VIEWPORT_PROPERTIES: [&str; 4] = [
    "--visual-viewport-width",
    "--visual-viewport-height",
    "--visual-viewport-left",
    "--visual-viewport-top",
];

#[cfg(target_arch = "wasm32")]
struct ViewportGuard {
    viewport: web_sys::VisualViewport,
    root: web_sys::HtmlElement,
    callback: Closure<dyn FnMut()>,
    previous: Vec<(&'static str, String, String)>,
}

#[cfg(target_arch = "wasm32")]
impl ViewportGuard {
    fn install() -> Option<Self> {
        let window = web_sys::window()?;
        let viewport = window.visual_viewport()?;
        let root =
            window.document()?.document_element()?.dyn_into::<web_sys::HtmlElement>().ok()?;
        let style = root.style();
        let previous = VIEWPORT_PROPERTIES
            .iter()
            .map(|&name| {
                (
                    name,
                    style.get_property_value(name).unwrap_or_default(),
                    style.get_property_priority(name),
                )
            })
            .collect();
        let callback_viewport = viewport.clone();
        let callback_root = root.clone();
        let callback = Closure::<dyn FnMut()>::new(move || {
            synchronize(&callback_viewport, &callback_root);
        });
        let guard = Self { viewport, root, callback, previous };
        for event in ["resize", "scroll"] {
            guard
                .viewport
                .add_event_listener_with_callback(event, guard.callback.as_ref().unchecked_ref())
                .ok()?;
        }
        synchronize(&guard.viewport, &guard.root);
        Some(guard)
    }
}

#[cfg(target_arch = "wasm32")]
fn synchronize(viewport: &web_sys::VisualViewport, root: &web_sys::HtmlElement) {
    let dimensions =
        [viewport.width(), viewport.height(), viewport.offset_left(), viewport.offset_top()];
    if dimensions.iter().any(|value| !value.is_finite())
        || dimensions[0] <= 0.0
        || dimensions[1] <= 0.0
    {
        return;
    }
    let style = root.style();
    for (name, value) in VIEWPORT_PROPERTIES.iter().zip(dimensions) {
        let value = format!("{value}px");
        if style.get_property_value(name).ok().as_deref() != Some(&value) {
            let _ = style.set_property(name, &value);
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for ViewportGuard {
    fn drop(&mut self) {
        for event in ["resize", "scroll"] {
            let _ = self
                .viewport
                .remove_event_listener_with_callback(event, self.callback.as_ref().unchecked_ref());
        }
        let style = self.root.style();
        for (name, value, priority) in &self.previous {
            if value.is_empty() {
                let _ = style.remove_property(name);
            } else {
                let _ = style.set_property_with_priority(name, value, priority);
            }
        }
    }
}
