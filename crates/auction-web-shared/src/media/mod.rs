//! 浏览器 `matchMedia` 响应式查询。
//! 供需要按断点切换组件结构的场景使用；纯样式切换仍应优先用 CSS `@media`，不经 WASM 往返。

use dioxus::prelude::*;

mod viewport;
pub(crate) use viewport::use_visual_viewport;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

/// 订阅一条 CSS media query，返回随匹配结果更新的信号。
/// 首次 effect 同步当前结果，之后经 `change` 监听更新；组件卸载时自动移除监听。
/// effect 执行前与非 WASM 构建（仅静态检查）的初值均为 `false`。
pub fn use_media_query(_query: String) -> Signal<bool> {
    let matches = use_signal(|| false);

    #[cfg(target_arch = "wasm32")]
    {
        let handle =
            use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(None::<MediaQueryListener>)));

        let effect_handle = std::rc::Rc::clone(&handle);
        use_effect(use_reactive((&_query,), move |(query,)| {
            let Some(window) = web_sys::window() else {
                return;
            };
            let Ok(Some(list)) = window.match_media(&query) else {
                return;
            };

            let mut matches = matches;
            matches.set(list.matches());

            let callback = Closure::<dyn FnMut(web_sys::MediaQueryListEvent)>::new(
                move |event: web_sys::MediaQueryListEvent| {
                    matches.set(event.matches());
                },
            );
            let _ =
                list.add_event_listener_with_callback("change", callback.as_ref().unchecked_ref());
            // query 变化触发的 effect 重跑会替换旧监听；drop guard 在替换/卸载时移除。
            effect_handle.borrow_mut().replace(MediaQueryListener { list, callback });
        }));

        use_drop(move || {
            handle.borrow_mut().take();
        });
    }

    matches
}

/// `change` 监听的生命周期守卫：effect 重跑或组件卸载时移除监听并释放闭包。
#[cfg(target_arch = "wasm32")]
struct MediaQueryListener {
    list: web_sys::MediaQueryList,
    callback: Closure<dyn FnMut(web_sys::MediaQueryListEvent)>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for MediaQueryListener {
    fn drop(&mut self) {
        let _ = self
            .list
            .remove_event_listener_with_callback("change", self.callback.as_ref().unchecked_ref());
    }
}
