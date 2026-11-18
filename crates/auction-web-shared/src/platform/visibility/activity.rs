//! 窗口内真实输入的被动监听与卸载。

use js_sys::{Function, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Element, Event, Window};

const INPUT_EVENTS: [&str; 4] = ["pointerdown", "keydown", "wheel", "touchstart"];

/// 窗口活动监听；释放时移除对应回调，不持有页面业务状态。
pub struct InputActivityListener {
    window: Window,
    callback: Closure<dyn FnMut(Event)>,
}

impl InputActivityListener {
    /// 通知调用方真实输入是否来自指定控件或其子元素。
    pub fn install(selector: String, mut on_activity: impl FnMut(bool) + 'static) -> Option<Self> {
        let window = web_sys::window()?;
        let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
            if event.is_trusted() {
                let matches = event.composed_path().iter().any(|target| {
                    target
                        .dyn_ref::<Element>()
                        .is_some_and(|element| element.matches(&selector).unwrap_or_default())
                });
                on_activity(matches);
            }
        });
        let listener = Self { window, callback };
        let add_listener = Reflect::get(&listener.window, &JsValue::from_str("addEventListener"))
            .ok()?
            .dyn_into::<Function>()
            .ok()?;
        let options = Object::new();
        Reflect::set(&options, &JsValue::from_str("passive"), &JsValue::TRUE).ok()?;
        // 捕获阶段监听，控件停止冒泡时仍可记下本次人工活动。
        Reflect::set(&options, &JsValue::from_str("capture"), &JsValue::TRUE).ok()?;
        for event in INPUT_EVENTS {
            add_listener
                .call3(
                    &listener.window,
                    &JsValue::from_str(event),
                    listener.callback.as_ref(),
                    &options,
                )
                .ok()?;
        }
        Some(listener)
    }
}

impl Drop for InputActivityListener {
    fn drop(&mut self) {
        for event in INPUT_EVENTS {
            let _ = self.window.remove_event_listener_with_callback_and_bool(
                event,
                self.callback.as_ref().unchecked_ref(),
                true,
            );
        }
    }
}
