//! 单持有者优先锁：同一时刻只有一个持有者，前台登记插到所有后台等待者之前，同类按登记顺序。
//! 排队状态机 `OperationLockState` 是纯计算，`OperationLock` 只负责登记、唤醒与取消。
//!
//! MoogleMail 有两种实例：网关按凭据持有的操作锁让 compose 与资产动作独占整次逻辑操作，长列表
//! 扫描不得登记（只读观测不改变远端成员关系，阻塞在锁上只会推迟写入）；客户端（克隆共享）的
//! 在途锁让物理请求串行，每次只持有一个请求，因此后台扫描逐个请求登记，前台写入在请求之间插队。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::clients::gate::GateClass;

/// 共享的单账号优先锁；克隆出的会话与客户端持有同一个实例。
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

/// 独占 guard；Drop 立即释放并唤醒全部等待者，由队首票据取得。操作锁不得跨长时间扫描持有。
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

/// 单持有者优先锁的纯排队状态；同一时刻只有一个持有者。操作锁与在途锁共用这一份状态机：
/// 前者覆盖整次 compose/资产逻辑操作，后者只覆盖单个物理请求。只决定先后，不决定业务结果，
/// 也不是租约或完成状态的来源。
#[derive(Debug, Default)]
struct OperationLockState {
    held: bool,
    next_ticket: u64,
    queue: VecDeque<(u64, GateClass)>,
}

impl OperationLockState {
    /// 前台排在所有后台之前；同类按登记顺序。
    fn register(&mut self, class: GateClass) -> u64 {
        let ticket = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1);
        match class {
            GateClass::Foreground => {
                let index = self
                    .queue
                    .iter()
                    .position(|(_, queued)| *queued == GateClass::Background)
                    .unwrap_or(self.queue.len());
                self.queue.insert(index, (ticket, class));
            }
            GateClass::Background => self.queue.push_back((ticket, class)),
        }
        ticket
    }

    /// 取消尚未获准的登记；等待者被丢弃时必须调用，避免空票据阻塞后续请求。
    fn cancel(&mut self, ticket: u64) {
        self.queue.retain(|(queued, _)| *queued != ticket);
    }

    /// 仅当锁空闲且该票据位于队首时获准。
    fn try_acquire(&mut self, ticket: u64) -> bool {
        if self.held || self.queue.front().map(|(queued, _)| *queued) != Some(ticket) {
            return false;
        }
        self.queue.pop_front();
        self.held = true;
        true
    }

    /// 释放唯一持有者；只有持锁 guard 的 Drop 可以调用。
    fn release(&mut self) {
        self.held = false;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{OperationLock, OperationLockState};
    use crate::clients::gate::GateClass;

    #[test]
    fn state_serializes_and_prioritizes_foreground() {
        let mut lock = OperationLockState::default();
        let background = lock.register(GateClass::Background);
        let foreground = lock.register(GateClass::Foreground);
        assert!(!lock.try_acquire(background));
        assert!(lock.try_acquire(foreground));
        assert!(!lock.try_acquire(background));
        lock.release();
        assert!(lock.try_acquire(background));
        assert!(lock.held);
        lock.release();
        assert!(!lock.held);
    }

    #[test]
    fn state_cancel_does_not_block_queue() {
        let mut lock = OperationLockState::default();
        let cancelled = lock.register(GateClass::Foreground);
        let next = lock.register(GateClass::Foreground);
        lock.cancel(cancelled);
        assert!(lock.try_acquire(next));
    }

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
