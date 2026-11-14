//! 页面可见性共享原语：隐藏期间轮询与重连退避挂起而非重置，恢复可见后继续。
//! Public 外壳与 Admin 自动轮询共用；是否在恢复可见时补偿刷新由调用方回调决定。

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    task::{Poll, Waker},
};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

use crate::timer::TimeoutFuture;

type WakerSlot = RefCell<Option<Waker>>;
type WakerRegistry = RefCell<Vec<Weak<WakerSlot>>>;

/// 页面可见性；`visibilitychange` 监听器更新内部状态并唤醒挂起的等待者。
/// 内部是 `Rc` 共享状态，克隆体与原体看到同一份可见性，供同一页面的多个循环共用。
#[derive(Clone)]
pub struct PageVisibility {
    hidden: Rc<Cell<bool>>,
    wakers: Rc<WakerRegistry>,
    #[cfg(target_arch = "wasm32")]
    listener: Rc<RefCell<Option<PageVisibilityListener>>>,
}

impl Default for PageVisibility {
    fn default() -> Self {
        Self::new()
    }
}

impl PageVisibility {
    pub fn new() -> Self {
        Self {
            hidden: Rc::new(Cell::new(false)),
            wakers: Rc::new(RefCell::new(Vec::new())),
            #[cfg(target_arch = "wasm32")]
            listener: Rc::new(RefCell::new(None)),
        }
    }

    /// 同步读取可见性；不建立额外监听或定时器。
    pub fn is_visible(&self) -> bool {
        !self.hidden.get()
    }

    /// 安装 `visibilitychange` 监听（仅浏览器目标）；恢复可见时先执行 `on_visible`，
    /// 再唤醒所有挂起的等待。与持有者同寿命，重复调用只安装一次。
    #[cfg(target_arch = "wasm32")]
    pub fn install(&self, mut on_visible: impl FnMut() + 'static) {
        if self.listener.borrow().is_some() {
            return;
        }
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        // 安装前先同步一次当前状态：监听器登记前页面可能已经是隐藏的。
        self.hidden.set(document.hidden());
        // 回调只持有状态，不能反向持有保存自身的 listener，否则最后一个外部持有者释放后仍成环。
        let hidden_state = Rc::clone(&self.hidden);
        let wakers = Rc::clone(&self.wakers);
        let callback_document = document.clone();
        let callback = Closure::<dyn FnMut()>::new(move || {
            let hidden = callback_document.hidden();
            hidden_state.set(hidden);
            if !hidden {
                on_visible();
                wake_waiters(&wakers);
            }
        });
        let _ = document.add_event_listener_with_callback(
            "visibilitychange",
            callback.as_ref().unchecked_ref(),
        );
        *self.listener.borrow_mut() = Some(PageVisibilityListener { document, callback });
    }

    /// 非浏览器目标没有隐藏事件，安装是无操作。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn install(&self, _on_visible: impl FnMut() + 'static) {}

    /// 卸载 `visibilitychange` 监听，避免路由反复挂载时监听器累积。
    pub fn cleanup(&self) {
        #[cfg(target_arch = "wasm32")]
        drop(self.listener.borrow_mut().take());
    }

    /// 页面隐藏时挂起直到恢复可见；当前已可见时立即返回，不改变既有计时节奏。
    pub async fn wait_until_visible(&self) {
        if !self.hidden.get() {
            return;
        }
        let waiter = VisibilityWaiter { wakers: &self.wakers, slot: Rc::new(RefCell::new(None)) };
        std::future::poll_fn(|context| {
            if self.hidden.get() {
                waiter.register(context.waker());
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }
}

/// 一个隐藏等待只登记一个槽位；取消 future 时立即解除登记，不等待页面再次可见。
struct VisibilityWaiter<'a> {
    wakers: &'a WakerRegistry,
    slot: Rc<WakerSlot>,
}

impl VisibilityWaiter<'_> {
    fn register(&self, waker: &Waker) {
        let registration = Rc::downgrade(&self.slot);
        let mut wakers = self.wakers.borrow_mut();
        // 唤醒后、任务再次 poll 前可能重新隐藏；此时需要重新登记同一槽位。
        if !wakers.iter().any(|known| known.ptr_eq(&registration)) {
            wakers.push(registration);
        }
        let mut slot = self.slot.borrow_mut();
        match slot.as_mut() {
            Some(known) => known.clone_from(waker),
            None => *slot = Some(waker.clone()),
        }
    }
}

impl Drop for VisibilityWaiter<'_> {
    fn drop(&mut self) {
        let registration = Rc::downgrade(&self.slot);
        self.wakers.borrow_mut().retain(|known| !known.ptr_eq(&registration));
    }
}

#[cfg(target_arch = "wasm32")]
struct PageVisibilityListener {
    document: web_sys::Document,
    callback: Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for PageVisibilityListener {
    fn drop(&mut self) {
        let _ = self.document.remove_event_listener_with_callback(
            "visibilitychange",
            self.callback.as_ref().unchecked_ref(),
        );
    }
}

// 先释放列表借用，再调用可能重新注册等待者的运行时回调。
#[cfg(target_arch = "wasm32")]
fn wake_waiters(wakers: &WakerRegistry) {
    let waiters = std::mem::take(&mut *wakers.borrow_mut());
    for waiter in waiters {
        if let Some(slot) = waiter.upgrade() {
            let waker = slot.borrow_mut().take();
            if let Some(waker) = waker {
                waker.wake();
            }
        }
    }
}

/// 分片睡眠：隐藏时挂起剩余时长（不重置计时），恢复可见后继续走完剩余分片。
/// 浏览器会大幅节流隐藏页面的定时器，事件驱动的可见性等待比单纯拉长 setTimeout 更可靠。
pub async fn sleep_while_visible(visibility: &PageVisibility, delay_millis: u32) {
    /// 睡眠分片粒度：足够细以便在隐藏时尽快转入事件驱动挂起，又不会引入明显唤醒开销。
    const SLEEP_SLICE_MILLIS: u32 = 1_000;
    let mut remaining = delay_millis;
    while remaining > 0 {
        visibility.wait_until_visible().await;
        let slice = remaining.min(SLEEP_SLICE_MILLIS);
        TimeoutFuture::new(slice).await;
        remaining -= slice;
    }
    // 最后一片计时期间也可能转入隐藏；不能在隐藏页完成等待并立即发起轮询或重连。
    visibility.wait_until_visible().await;
}
