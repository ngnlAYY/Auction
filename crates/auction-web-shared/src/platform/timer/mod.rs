//! 一次性定时器的 `Future` 封装：到时完成，提前释放即取消底层定时器。

use std::{
    cell::{Cell, RefCell},
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};

#[wasm_bindgen]
extern "C" {
    // 绑定全局函数而不是 `Window` 方法：句柄在浏览器里是数字，在其他 JS 运行时可能是对象。
    #[wasm_bindgen(js_name = "setTimeout")]
    fn set_timeout(handler: &js_sys::Function, milliseconds: i32) -> JsValue;

    #[wasm_bindgen(js_name = "clearTimeout")]
    fn clear_timeout(handle: &JsValue);
}

/// 经过指定毫秒后完成的等待。
///
/// 在竞速中落败或所属任务被取消时随值释放，底层定时器同时取消，不会留下迟到的回调。
#[must_use = "futures do nothing unless polled or awaited"]
pub struct TimeoutFuture {
    state: Rc<TimeoutState>,
    handle: JsValue,
    // 回调必须活到触发或取消之后；先释放会让运行时调用已失效的闭包。
    _callback: Closure<dyn FnMut()>,
}

#[derive(Default)]
struct TimeoutState {
    fired: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl TimeoutFuture {
    pub fn new(milliseconds: u32) -> Self {
        let state = Rc::new(TimeoutState::default());
        let fired_state = Rc::clone(&state);
        let callback = Closure::once(move || {
            fired_state.fired.set(true);
            // 先结束借用再唤醒，唤醒方即使同步轮询也不会重入同一个 `RefCell`。
            let waker = fired_state.waker.borrow_mut().take();
            if let Some(waker) = waker {
                waker.wake();
            }
        });
        // 超过 `i32` 的延时会被运行时当作 0；饱和到上限保持“至少等这么久”。
        let handle = set_timeout(
            callback.as_ref().unchecked_ref(),
            i32::try_from(milliseconds).unwrap_or(i32::MAX),
        );
        Self { state, handle, _callback: callback }
    }
}

impl Future for TimeoutFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.state.fired.get() {
            return Poll::Ready(());
        }
        // 每次都换成最新的 waker：同一等待可能在竞速组合器之间被不同任务轮询。
        *self.state.waker.borrow_mut() = Some(context.waker().clone());
        Poll::Pending
    }
}

impl Drop for TimeoutFuture {
    fn drop(&mut self) {
        // 已触发的句柄再取消是无操作，不需要区分。
        clear_timeout(&self.handle);
    }
}
