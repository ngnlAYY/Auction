//! 秒级共享时钟。
//! 外壳创建单一 interval 并经 context 下发，消费方订阅同一计数信号，避免同屏多个
//! `TimeoutFuture` 一秒循环叠加；本 crate 不引入服务端时钟校准业务。

#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

/// 外壳持有的秒级时钟句柄，由 [`use_second_ticker`] 创建并提供给子树。
#[derive(Clone)]
pub struct SecondTicker {
    tick: Signal<u64>,
    #[cfg(target_arch = "wasm32")]
    state: Rc<RefCell<TickerState>>,
}

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct TickerState {
    /// 当前订阅者数量；从 0 到 1 时启动 interval，回到 0 时停止。
    subscribers: usize,
    interval: Option<IntervalGuard>,
}

#[cfg(target_arch = "wasm32")]
struct IntervalGuard {
    window: web_sys::Window,
    handle: i32,
    _callback: Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for IntervalGuard {
    fn drop(&mut self) {
        self.window.clear_interval_with_handle(self.handle);
    }
}

/// 在外壳根组件调用一次：创建秒级时钟并通过 context 提供给整棵子树。
/// interval 惰性：首个订阅者挂载时启动，最后一个订阅者卸载后停止。
pub fn use_second_ticker() -> SecondTicker {
    let ticker = use_hook(|| SecondTicker {
        tick: Signal::new(0),
        #[cfg(target_arch = "wasm32")]
        state: Rc::new(RefCell::new(TickerState::default())),
    });
    use_context_provider(|| ticker.clone());
    ticker
}

/// 订阅外壳提供的秒级时钟，返回每秒递增一次的修订计数信号。
/// 返回值是不透明修订号，不代表具体时间戳；消费方在派生计算里读取它即可每秒重跑，
/// 「当前时间」仍由消费方各自的时钟来源求值。必须在 [`use_second_ticker`] 的 context 下调用。
pub fn use_second_tick() -> Signal<u64> {
    let ticker = use_context::<SecondTicker>();

    #[cfg(target_arch = "wasm32")]
    {
        let state = Rc::clone(&ticker.state);
        let tick = ticker.tick;
        use_hook(move || {
            let guard = Rc::new(SubscriptionGuard { state: Rc::clone(&state) });
            let mut borrowed = state.borrow_mut();
            borrowed.subscribers = borrowed.subscribers.saturating_add(1);
            if borrowed.subscribers == 1 && borrowed.interval.is_none() {
                start_interval(&mut borrowed, tick);
            }
            drop(borrowed);
            // Dioxus 在组件卸载时 drop hook 返回值；Rc 使 hook 内外的克隆共享同一守卫，
            // 最后一个引用消失时才触发 Drop 退订。
            guard
        });
    }

    ticker.tick
}

/// 订阅守卫：组件卸载时减少订阅计数，归零即停止 interval。
#[cfg(target_arch = "wasm32")]
struct SubscriptionGuard {
    state: Rc<RefCell<TickerState>>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.subscribers = state.subscribers.saturating_sub(1);
        if state.subscribers == 0 {
            state.interval = None;
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn start_interval(state: &mut TickerState, tick: Signal<u64>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let callback = Closure::<dyn FnMut()>::new(move || {
        let mut tick = tick;
        tick.set(tick().saturating_add(1));
    });
    if let Ok(handle) = window.set_interval_with_callback_and_timeout_and_arguments_0(
        callback.as_ref().unchecked_ref(),
        1_000,
    ) {
        state.interval = Some(IntervalGuard { window, handle, _callback: callback });
    }
}
