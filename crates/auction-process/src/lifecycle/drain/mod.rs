//! 通用逐工作项排空；只管理内存所有权，不理解持久状态或业务种类。

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use tokio::sync::Notify;

const PREPARING: u8 = 0;
// CRITICAL 同时表示已登记的交接和关键段，两者均计数并阻止排空结束。
// 登记必须先于持久围栏调用；该状态不授权外写，调用方仍须确认围栏提交后才能 POST。
const CRITICAL: u8 = 1;
const FINISHED: u8 = 2;

/// 装配根负责日志和指标；false 仅表示活动释放时未确认本地结果。
pub trait DrainActivityObserver: Send + Sync {
    /// 活动首次登记交接或关键段，计数已生效。
    fn entered(&self);
    /// 活动释放；未确认提交只记录未收口，不能推断业务完成。
    fn released(&self, persisted: bool);
}

struct Shared {
    closing: AtomicBool,
    admission: Mutex<()>,
    active: AtomicUsize,
    cancel: Arc<Notify>,
    idle: Notify,
}

/// 一个进程共用一个协调器；关闭后不再允许新的关键段进入。
#[derive(Clone)]
pub struct DrainCoordinator(Arc<Shared>);
impl Default for DrainCoordinator {
    fn default() -> Self {
        Self(Arc::new(Shared {
            closing: AtomicBool::new(false),
            admission: Mutex::new(()),
            active: AtomicUsize::new(0),
            cancel: Arc::new(Notify::new()),
            idle: Notify::new(),
        }))
    }
}
impl DrainCoordinator {
    /// 同步关闭入口并唤醒所有准备期工作；已登记交接与关键段继续由原 owner 驱动。
    pub fn close(&self) {
        let admission = self.0.admission.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.0.closing.store(true, Ordering::Release);
        drop(admission);
        self.0.cancel.notify_waiters();
    }
    /// 注册叶子活动；关闭之后创建的活动仍不能进入关键段。
    pub fn activity(&self, observer: Arc<dyn DrainActivityObserver>) -> DrainActivity {
        DrainActivity(Arc::new(Activity {
            shared: Arc::clone(&self.0),
            phase: AtomicU8::new(PREPARING),
            observer,
            cancelled: Mutex::new(Box::pin(Arc::clone(&self.0.cancel).notified_owned())),
        }))
    }
    /// 调用方先 close；交接和关键段均结束后零活动才是稳定结果，不以计数代替 owner 的 join。
    pub async fn wait_critical(&self) {
        loop {
            let changed = self.0.idle.notified();
            if self.0.active.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }
}

struct Activity {
    shared: Arc<Shared>,
    phase: AtomicU8,
    observer: Arc<dyn DrainActivityObserver>,
    cancelled: Mutex<Pin<Box<dyn Future<Output = ()> + Send>>>,
}
impl Activity {
    fn release(&self, persisted: bool) {
        // complete 与进入关键段共用入口锁，避免克隆间交接时计数先减后增。
        let admission =
            self.shared.admission.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let was_critical = self.phase.swap(FINISHED, Ordering::AcqRel) == CRITICAL;
        if was_critical {
            self.shared.active.fetch_sub(1, Ordering::AcqRel);
        }
        drop(admission);
        if was_critical {
            self.observer.released(persisted);
            self.shared.idle.notify_waiters();
        }
    }
    fn is_cancelled(&self) -> bool {
        self.shared.closing.load(Ordering::Acquire)
            && self.phase.load(Ordering::Acquire) != CRITICAL
    }
}
impl Drop for Activity {
    fn drop(&mut self) {
        self.release(false);
    }
}

/// 最后一个克隆释放时才回收活动；drop 从不声称持久化成功。
#[derive(Clone)]
pub struct DrainActivity(Arc<Activity>);
impl DrainActivity {
    /// 围栏前交接与关键段共用登记，与关闭入口同步线性化；后续步骤沿用同一守卫。
    /// 已登记只表示排空会等待，持久围栏确认仍由调用方负责，不在这里授予外写权限。
    pub fn enter_critical(&self) -> bool {
        let admission =
            self.0.shared.admission.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let phase = self.0.phase.load(Ordering::Acquire);
        if phase == CRITICAL {
            return true;
        }
        if phase == FINISHED || self.0.shared.closing.load(Ordering::Acquire) {
            return false;
        }
        self.0.phase.store(CRITICAL, Ordering::Release);
        self.0.shared.active.fetch_add(1, Ordering::AcqRel);
        drop(admission);
        self.0.observer.entered();
        true
    }
    /// 调用方已经确认原结果提交；只释放进程内计数。
    pub fn complete(&self) {
        self.0.release(true);
    }
    /// 只有入口已关闭且活动尚未登记交接或关键段时才取消。
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    /// 注册准备期取消唤醒；已登记交接与关键段仍由原 future 的唤醒继续驱动。
    pub fn poll_cancelled(&self, context: &mut Context<'_>) -> Poll<()> {
        // 已登记的交接与关键段只受外层共享截止和二次信号约束，不再 poll 关闭通知。
        if self.0.phase.load(Ordering::Acquire) == CRITICAL {
            return Poll::Pending;
        }
        if self.is_cancelled() {
            return Poll::Ready(());
        }
        let mut cancelled =
            self.0.cancelled.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _notification = cancelled.as_mut().poll(context);
        // 关闭和本次 poll 可交错；再次检查，不能漏掉关闭唤醒或取消已进入的关键段。
        if self.is_cancelled() { Poll::Ready(()) } else { Poll::Pending }
    }
}
