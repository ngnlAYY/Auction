//! MoogleMail Worker 的模块装配；远程读取、维护与节奏各自拥有独立状态，
//! 不共享游标，也不把外部调用带进调度层。

pub(super) mod maintenance;
pub(super) mod poll;
pub(super) mod schedule;

use self::poll::MailPollRuntime;
use crate::worker::runtime::PersistentWorker;

/// 尝试取得维护互斥锁；停机或排空时不排队，取得后再复核一次。
pub(super) async fn acquire_maintenance_lock<'a, C>(
    worker: &PersistentWorker<C>,
    runtime: &'a MailPollRuntime,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> Option<tokio::sync::MutexGuard<'a, ()>> {
    if *shutdown.borrow() || worker.control.is_draining() {
        return None;
    }
    tokio::select! {
        biased;
        changed = shutdown.changed() => {
            drop(changed);
            None
        }
        guard = runtime.maintenance_lock.lock() => {
            (!worker.control.is_draining() && !*shutdown.borrow()).then_some(guard)
        }
    }
}
