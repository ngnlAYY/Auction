//! 部署后旧页面的构建号偏差检测。
//!
//! 服务端在每个 HTTP 响应头 `x-auction-build` 与实时 WebSocket 首帧报告进程构建号；浏览器
//! 只把它与自身 [`build_id`] 比较，不一致时提示刷新，不自动重载正在操作的页面。检测只使用
//! 原子状态与一次性唤醒，逐请求比较成功路径不取锁。

use std::{
    future::Future,
    pin::Pin,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
};

/// 服务端报告构建号的响应头；与 `auction-http` 的 `x-auction-build` 保持同一契约。
pub const SERVER_BUILD_HEADER: &str = "x-auction-build";

// 构建号与信标共用同一份值：发布构建通过 `AUCTION_BUILD_ID` 注入，非法注入回退包版本。
pub use crate::bootstrap::build_id;

/// 已发现与自身不同的服务器构建号；一旦置位保持到页面刷新。
static STALE: AtomicBool = AtomicBool::new(false);

/// 等待偏差出现的登记表；只在首次置位时取锁并清空，正常比较路径不触碰。
static WAITERS: LazyLock<Mutex<Vec<(usize, Waker)>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static NEXT_WAITER_ID: AtomicUsize = AtomicUsize::new(0);

/// 比较服务端报告值；返回是否与自身构建号不一致。相同值走无锁快路径。
pub fn observe_server_build_id(reported: &str) -> bool {
    if reported == build_id() {
        return false;
    }
    mark_stale();
    true
}

/// 已发现构建号偏差。
pub fn build_is_stale() -> bool {
    STALE.load(Ordering::Acquire)
}

fn waiters() -> std::sync::MutexGuard<'static, Vec<(usize, Waker)>> {
    WAITERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 首次置位时唤醒全部等待者；重复置位不重复取锁。
fn mark_stale() {
    if STALE.swap(true, Ordering::AcqRel) {
        return;
    }
    let wakers = std::mem::take(&mut *waiters());
    for (_, waker) in wakers {
        waker.wake();
    }
}

/// 等待构建号偏差出现的一次性 future；已偏差时立即就绪。
#[derive(Debug)]
pub struct BuildDriftWait {
    id: usize,
    finished: bool,
}

/// 登记一个等待者；只用于组件外壳的一次性订阅。
pub fn wait_for_build_drift() -> BuildDriftWait {
    BuildDriftWait { id: NEXT_WAITER_ID.fetch_add(1, Ordering::Relaxed), finished: false }
}

impl Future for BuildDriftWait {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.finished || build_is_stale() {
            self.finished = true;
            return Poll::Ready(());
        }
        let mut registered = waiters();
        // 登记前后都可能发生置位，持锁再检查一次避免丢失唤醒。
        if build_is_stale() {
            self.finished = true;
            return Poll::Ready(());
        }
        match registered.iter_mut().find(|(id, _)| *id == self.id) {
            Some((_, waker)) => {
                if !waker.will_wake(context.waker()) {
                    *waker = context.waker().clone();
                }
            }
            None => registered.push((self.id, context.waker().clone())),
        }
        Poll::Pending
    }
}

impl Drop for BuildDriftWait {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        waiters().retain(|(id, _)| *id != self.id);
    }
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    STALE.store(false, Ordering::Release);
    waiters().clear();
}

use dioxus::prelude::*;

/// 订阅构建号偏差；已偏差时立即为 true，之后保持到页面刷新。
/// 只登记一个一次性等待，不轮询、不改动正在编辑的页面状态。
pub fn use_build_drift() -> Signal<bool> {
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
    let mut stale = use_signal(build_is_stale);
    #[cfg(target_arch = "wasm32")]
    use_future(move || async move {
        if build_is_stale() {
            return;
        }
        wait_for_build_drift().await;
        stale.set(true);
    });
    stale
}

/// 部署后旧页面的刷新提示；文案由各 surface 提供，不使用浏览器的自动刷新。
#[component]
pub fn StaleBuildBanner(message: String, refresh_label: String) -> Element {
    let stale = use_build_drift();
    if !*stale.read() {
        return rsx! {};
    }
    rsx! {
        div { class: "shared-stale-build", role: "status",
            p { class: "shared-stale-build__message", "{message}" }
            button {
                r#type: "button",
                class: "shared-stale-build__refresh",
                onclick: move |_| reload_page(),
                "{refresh_label}"
            }
        }
    }
}

fn reload_page() {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        // 只有用户显式点击才重载，避免打断正在进行的操作。
        let _ = window.location().reload();
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::build_notice as parent;

    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    fn poll(wait: &mut BuildDriftWait) -> Poll<()> {
        let waker = futures_util::task::noop_waker();
        let mut context = Context::from_waker(&waker);
        Pin::new(wait).poll(&mut context)
    }

    /// 全局偏差状态由整个进程共享，全部断言放在同一个用例内避免并行测试互相重置。
    #[test]
    fn build_drift_notice_tracks_the_first_mismatch() {
        reset_for_tests();
        assert_eq!(SERVER_BUILD_HEADER, "x-auction-build");

        // 相同构建号保持静默，登记后的等待保持挂起，丢弃时注销。
        assert!(!observe_server_build_id(build_id()));
        assert!(!build_is_stale());
        {
            let mut wait = wait_for_build_drift();
            assert!(matches!(poll(&mut wait), Poll::Pending));
        }

        let mut wait = wait_for_build_drift();
        assert!(matches!(poll(&mut wait), Poll::Pending));
        assert!(observe_server_build_id("build_other"));
        assert!(build_is_stale());
        assert!(matches!(poll(&mut wait), Poll::Ready(())));
        // 已置位后新登记的等待立即就绪，重复报告保持同一状态。
        assert!(matches!(poll(&mut wait_for_build_drift()), Poll::Ready(())));
        assert!(observe_server_build_id("build_other"));
        assert!(build_is_stale());
        reset_for_tests();
        assert!(!build_is_stale());
        assert!(matches!(poll(&mut wait_for_build_drift()), Poll::Pending));
        reset_for_tests();
    }
}
