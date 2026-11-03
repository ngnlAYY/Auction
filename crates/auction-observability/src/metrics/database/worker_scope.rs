//! 仅传播 WorkerLoop 的任务局部归属；jobs 诊断 scope/inherit 共用同一循环身份。

use std::future::Future;

use crate::WorkerLoop;

tokio::task_local! {
    static DATABASE_WORKER_LOOP: WorkerLoop;
}

/// 为一个 future 设置固定循环身份；新任务须由调用方显式继承，不产生后台任务。
pub async fn scope_database_worker_loop<F: Future>(loop_kind: WorkerLoop, future: F) -> F::Output {
    DATABASE_WORKER_LOOP.scope(loop_kind, future).await
}

/// 获取当前循环；心跳、开关刷新和其他未归属任务返回 None，不伪装为业务循环。
pub fn current_database_worker_loop() -> Option<WorkerLoop> {
    DATABASE_WORKER_LOOP.try_with(|value| *value).ok()
}
