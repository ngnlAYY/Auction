//! Admin 运维页面的可选自动轮询：开关与间隔状态由页面持有（入 URL query），本模块提供
//! 页面可见性感知、分片睡眠、轮询调度 hook 与开关控件。
//! 每个页面拥有独立的可见性监听，卸载时释放回调；开关和轮询代数不改变其它页面的状态。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::task::{Poll, Waker};

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

/// 可选的自动轮询间隔（秒）；0 表示关闭。与 URL query `poll` 键的合法值一一对应。
pub(crate) const POLL_INTERVAL_VALUES: &[u64] = &[10, 30, 60];

/// 页面可见性；`visibilitychange` 监听器更新内部状态并唤醒挂起的轮询等待者。
/// 内部是 `Rc` 共享状态，克隆体与原体看到同一份可见性，供页面内多个轮询循环共用。
#[derive(Clone)]
pub(crate) struct PageVisibility {
    hidden: Rc<Cell<bool>>,
    wakers: Rc<RefCell<Vec<Waker>>>,
    #[cfg(target_arch = "wasm32")]
    listener: Rc<RefCell<Option<PageVisibilityListener>>>,
}

impl PageVisibility {
    pub(crate) fn new() -> Self {
        Self {
            hidden: Rc::new(Cell::new(false)),
            wakers: Rc::new(RefCell::new(Vec::new())),
            #[cfg(target_arch = "wasm32")]
            listener: Rc::new(RefCell::new(None)),
        }
    }

    /// 安装 `visibilitychange` 监听（仅浏览器目标）；恢复可见时唤醒所有挂起的轮询等待。
    /// 与页面组件同寿命，只安装一次。
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn install(&self) {
        if self.listener.borrow().is_some() {
            return;
        }
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        // 安装前先同步一次当前状态：监听器登记前页面可能已经是隐藏的。
        self.hidden.set(document.hidden());
        let state = self.clone();
        let callback_document = document.clone();
        let callback = Closure::<dyn FnMut()>::new(move || {
            let hidden = callback_document.hidden();
            state.hidden.set(hidden);
            if !hidden {
                state.wake_waiters();
            }
        });
        let _ = document.add_event_listener_with_callback(
            "visibilitychange",
            callback.as_ref().unchecked_ref(),
        );
        *self.listener.borrow_mut() = Some(PageVisibilityListener { document, callback });
    }

    /// 卸载页面可见性监听，避免路由反复挂载时监听器累积。
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn cleanup(&self) {
        if let Some(listener) = self.listener.borrow_mut().take() {
            let _ = listener.document.remove_event_listener_with_callback(
                "visibilitychange",
                listener.callback.as_ref().unchecked_ref(),
            );
        }
    }

    /// 非浏览器目标没有隐藏事件，安装是无操作。
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn install(&self) {}

    // 仅 visibilitychange 监听器（wasm）会主动唤醒；非浏览器目标没有隐藏事件。
    #[cfg(target_arch = "wasm32")]
    fn wake_waiters(&self) {
        // 先释放列表借用，再调用可能重新注册等待者的运行时回调。
        let wakers = std::mem::take(&mut *self.wakers.borrow_mut());
        for waker in wakers {
            waker.wake();
        }
    }

    /// 页面隐藏时挂起直到恢复可见；当前已可见时立即返回，不改变既有计时节奏。
    pub(crate) async fn wait_until_visible(&self) {
        if !self.hidden.get() {
            return;
        }
        std::future::poll_fn(|context| {
            if self.hidden.get() {
                self.wakers.borrow_mut().push(context.waker().clone());
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }
}

#[cfg(target_arch = "wasm32")]
struct PageVisibilityListener {
    document: web_sys::Document,
    callback: Closure<dyn FnMut()>,
}

/// 分片睡眠：隐藏时挂起剩余时长（不重置计时），恢复可见后继续走完剩余分片。
/// 浏览器会大幅节流隐藏页面的定时器，事件驱动的可见性等待比单纯拉长 setTimeout 更可靠。
pub(crate) async fn sleep_while_visible(visibility: &PageVisibility, delay_millis: u32) {
    /// 睡眠分片粒度：足够细以便在隐藏时尽快转入事件驱动挂起，又不会引入明显唤醒开销。
    const SLEEP_SLICE_MILLIS: u32 = 1_000;
    let mut remaining = delay_millis;
    while remaining > 0 {
        visibility.wait_until_visible().await;
        let slice = remaining.min(SLEEP_SLICE_MILLIS);
        TimeoutFuture::new(slice).await;
        remaining -= slice;
    }
}

/// 可选自动轮询 hook：间隔为 0 时关闭；开启后每隔 `interval_seconds` 秒执行一次
/// `on_poll`。页面隐藏期间计时挂起而非继续打请求，恢复可见后走完剩余分片继续轮询。
/// 间隔变化（含关闭）时旧循环在下个分片结束（≤1 秒）经代数检查后自行退出，
/// 不会与新一轮循环叠加计时器，也不会补发一次过期轮询。
pub(crate) fn use_auto_poll(interval_seconds: Memo<u64>, on_poll: impl FnMut() + Clone + 'static) {
    let visibility = use_hook(PageVisibility::new);
    // 只安装一次可见性监听；effect 不读取任何信号，挂载后不会重跑。
    use_effect({
        let visibility = visibility.clone();
        move || visibility.install()
    });
    #[cfg(target_arch = "wasm32")]
    {
        let cleanup_visibility = visibility.clone();
        use_drop(move || cleanup_visibility.cleanup());
    }
    // 轮询代数：每次间隔变化自增，旧循环检测到代数过期即退出。
    let mut generation = use_signal(|| 0_u64);
    use_effect(move || {
        let seconds = interval_seconds();
        let current = generation.peek().saturating_add(1);
        generation.set(current);
        if seconds == 0 {
            return;
        }
        let delay_millis = u32::try_from(seconds.saturating_mul(1_000)).unwrap_or(u32::MAX);
        let visibility = visibility.clone();
        let mut on_poll = on_poll.clone();
        spawn(async move {
            loop {
                sleep_while_visible(&visibility, delay_millis).await;
                // 开关关闭或间隔调整后，旧循环不再补发轮询。
                if *generation.peek() != current {
                    break;
                }
                on_poll();
            }
        });
    });
}

/// 自动轮询开关控件：单个下拉，默认关闭，可选 10/30/60 秒；变更由页面写回 URL query。
#[component]
pub(crate) fn AutoPollControls(poll_seconds: u64, on_change: EventHandler<u64>) -> Element {
    rsx! {
        label { class: "admin-poll-control",
            {t!("admin-poll-label")}
            select {
                value: poll_seconds.to_string(),
                onchange: move |event| {
                    // 下拉只提供合法闭集；解析失败保持现状。
                    if let Ok(seconds) = event.value().parse::<u64>() {
                        on_change.call(seconds);
                    }
                },
                option { value: "0", {t!("admin-poll-off")} }
                option { value: "10", {t!("admin-poll-interval", seconds: 10)} }
                option { value: "30", {t!("admin-poll-interval", seconds: 30)} }
                option { value: "60", {t!("admin-poll-interval", seconds: 60)} }
            }
        }
    }
}
