//! 维护阶段的指标守卫；排空超时或任务取消时仍记录 Canceled，不产生业务收件事件。

use std::{sync::Arc, time::Instant};

use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult};

/// 在途阶段在取消时仍记录一次 Canceled，正常结束后以实际结果记录一次。
pub(super) struct IterationGuard {
    metrics: Arc<dyn WorkerRecorder>,
    loop_kind: WorkerLoop,
    started: Instant,
    result: WorkerResult,
}

impl IterationGuard {
    /// 在首个数据库或远端操作前开始计时。
    pub(super) fn new(metrics: Arc<dyn WorkerRecorder>, loop_kind: WorkerLoop) -> Self {
        Self { metrics, loop_kind, started: Instant::now(), result: WorkerResult::Canceled }
    }

    /// 消费守卫并记录阶段结果；未调用时由 Drop 保留 Canceled 口径。
    pub(super) fn finish(mut self, result: WorkerResult) {
        self.result = result;
    }
}

impl Drop for IterationGuard {
    fn drop(&mut self) {
        self.metrics.observe_iteration(self.loop_kind, self.result, self.started.elapsed());
    }
}
