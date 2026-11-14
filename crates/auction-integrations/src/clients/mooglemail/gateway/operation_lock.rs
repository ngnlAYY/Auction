//! 独占操作锁：compose 与资产动作同一时刻只有一个持有者，前台写入排在后台维护之前。
//! 长列表扫描不得登记这把锁；只读观测不改变远端成员关系，阻塞在锁上只会推迟写入。

use std::sync::{Arc, Mutex};

use auction_application::mail::{GateClass, OperationLockState};
use tokio::sync::Notify;

/// 共享的单账号操作锁；克隆出的会话持有同一个实例。
pub(crate) struct OperationLock {
    state: Mutex<OperationLockState>,
    notify: Notify,
}

impl OperationLock {
    pub(crate) fn new() -> Self {
        Self { state: Mutex::new(OperationLockState::default()), notify: Notify::new() }
    }

    /// 按优先级排队取得唯一持有权；等待者被丢弃时同步撤销登记。
    pub(crate) async fn lock(self: &Arc<Self>, class: GateClass) -> OperationLockGuard {
        let ticket = self.with_state(|state| state.register(class));
        let mut registration = Registration { lock: self, ticket, armed: true };
        loop {
            // 先登记等待者再复查条件；释放使用 notify_waiters，不存在丢失唤醒。
            let notified = self.notify.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            if self.with_state(|state| state.try_acquire(ticket)) {
                registration.armed = false;
                return OperationLockGuard { lock: Arc::clone(self) };
            }
            notified.await;
        }
    }

    fn with_state<T>(&self, run: impl FnOnce(&mut OperationLockState) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        run(&mut state)
    }
}

impl Default for OperationLock {
    fn default() -> Self {
        Self::new()
    }
}

/// 独占 guard；Drop 立即释放并唤醒一个等待者。不能跨 `.await` 之外的长时间扫描持有。
pub(crate) struct OperationLockGuard {
    lock: Arc<OperationLock>,
}

impl Drop for OperationLockGuard {
    fn drop(&mut self) {
        self.lock.with_state(OperationLockState::release);
        self.lock.notify.notify_waiters();
    }
}

struct Registration<'a> {
    lock: &'a OperationLock,
    ticket: u64,
    armed: bool,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.lock.with_state(|state| state.cancel(self.ticket));
            // 被取消的等待者可能原本排在队首，必须让后续登记重新检查。
            self.lock.notify.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn exclusive_lock_serializes_holders() {
        let lock = Arc::new(OperationLock::new());
        let first = lock.lock(GateClass::Foreground).await;
        let (started, wait) = tokio::sync::oneshot::channel();
        let second_lock = Arc::clone(&lock);
        let second = tokio::spawn(async move {
            let _ = started.send(());
            let _guard = second_lock.lock(GateClass::Foreground).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        });
        wait.await.expect("second waiter started");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!second.is_finished(), "second holder must wait for the first guard");
        drop(first);
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("second guard acquired after release")
            .expect("second task completed");
    }

    #[tokio::test]
    async fn foreground_waiter_precedes_background() {
        let lock = Arc::new(OperationLock::new());
        let held = lock.lock(GateClass::Foreground).await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let background_lock = Arc::clone(&lock);
        let background_order = Arc::clone(&order);
        let background = tokio::spawn(async move {
            let _guard = background_lock.lock(GateClass::Background).await;
            background_order.lock().expect("order").push("background");
        });
        // 等后台先登记，再登记前台；前台必须插到它前面。
        tokio::time::sleep(Duration::from_millis(20)).await;
        let foreground_lock = Arc::clone(&lock);
        let foreground_order = Arc::clone(&order);
        let foreground = tokio::spawn(async move {
            let _guard = foreground_lock.lock(GateClass::Foreground).await;
            foreground_order.lock().expect("order").push("foreground");
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(held);
        tokio::time::timeout(Duration::from_secs(1), async {
            foreground.await.expect("foreground task");
            background.await.expect("background task");
        })
        .await
        .expect("both waiters completed");
        assert_eq!(*order.lock().expect("order"), vec!["foreground", "background"]);
    }
}
