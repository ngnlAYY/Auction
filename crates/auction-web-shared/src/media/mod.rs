//! 浏览器 `matchMedia` 响应式查询。
//!
//! 供需要根据断点或媒体特征切换组件结构的场景使用（纯样式切换仍应优先用 CSS
//! `@media`，不经由本 hook 往返 WASM）。

use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

/// 订阅一条 CSS media query，返回随匹配结果实时更新的信号。
///
/// 首次渲染同步给出当前匹配结果，之后通过 `change` 监听更新；组件卸载时自动移除
/// 监听。非 WASM 构建（仅服务于静态检查）返回恒为 `false` 的信号。
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

/// `change` 监听的生命周期守卫：随 effect 重跑或组件卸载移除监听并释放闭包。
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
