//! Public 页面可见性共享状态：隐藏期间轮询与实时重连退避挂起而非重置，恢复可见后继续。

use super::*;
use std::task::{Poll, Waker};

/// 页面可见性；`visibilitychange` 监听器更新内部状态并唤醒挂起的轮询/退避等待者。
/// 内部是 `Rc` 共享状态，克隆体与原体看到同一份可见性，供 router 轮询与实时 owner 共用。
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

    /// 安装 `visibilitychange` 监听（仅浏览器目标）；恢复可见时先执行 `on_visible`
    /// 补偿一次权威重读，再唤醒所有挂起的轮询/退避。与 PublicShell 同寿命，只安装一次。
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn install(&self, mut on_visible: impl FnMut() + 'static) {
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
                on_visible();
                state.wake_waiters();
            }
        });
        let _ = document.add_event_listener_with_callback(
            "visibilitychange",
            callback.as_ref().unchecked_ref(),
        );
        *self.listener.borrow_mut() = Some(PageVisibilityListener { document, callback });
    }

    /// 卸载 `visibilitychange` 监听，确保 PublicShell 销毁后不保留浏览器回调。
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn cleanup(&self) {
        if let Some(listener) = self.listener.borrow_mut().take() {
            let _ = listener.document.remove_event_listener_with_callback(
                "visibilitychange",
                listener.callback.as_ref().unchecked_ref(),
            );
        }
    }

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
        futures_util::future::poll_fn(|context| {
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

impl Default for PageVisibility {
    fn default() -> Self {
        Self::new()
    }
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
