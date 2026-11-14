//! MoogleMail Worker 的模块装配；远程读取、维护与节奏各自拥有独立状态，
//! 不共享游标，也不把外部调用带进调度层。

pub(super) mod maintenance;
pub(super) mod poll;
pub(super) mod schedule;

use tokio::sync::{MutexGuard, OwnedSemaphorePermit};

use self::poll::MailPollRuntime;
use crate::worker::runtime::PersistentWorker;

/// 一轮维护持有的资源：共享批量连接配额许可与世界维护锁。
pub(super) struct MaintenanceRound<'a> {
    /// 只覆盖本轮数据库工作；独立维护任务在只读观察前提前释放。
    pub(super) bulk_permit: Option<OwnedSemaphorePermit>,
    pub(super) guard: MutexGuard<'a, ()>,
}

/// 取得一轮维护所需的许可与世界维护锁；停机或排空时不排队，取得后再复核一次。
///
/// 全局加锁顺序：先取批量连接配额许可，再取世界维护锁；持有维护锁时绝不等待许可。收件
/// 旧路径与独立维护任务若顺序相反，配额只有 1 时在隔离开关切换窗口内会互相等待（一方持
/// 许可等锁、另一方持锁等许可），并连带堵住共享配额的全部批量循环，直到停止信号才解开。
pub(super) async fn acquire_maintenance_round<'a, C>(
    worker: &PersistentWorker<C>,
    runtime: &'a MailPollRuntime,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> Option<MaintenanceRound<'a>> {
    if *shutdown.borrow() || worker.control.is_draining() {
        return None;
    }
    let bulk_permit = tokio::select! {
        biased;
        changed = shutdown.changed() => {
            drop(changed);
            return None;
        }
        bulk_permit = worker.bulk_permit() => bulk_permit,
    };
    let guard = tokio::select! {
        biased;
        changed = shutdown.changed() => {
            drop(changed);
            return None;
        }
        guard = runtime.maintenance_lock.lock() => guard,
    };
    (!worker.control.is_draining() && !*shutdown.borrow())
        .then_some(MaintenanceRound { bulk_permit, guard })
}

#[cfg(test)]
mod tests;
