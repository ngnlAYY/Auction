//! 维护阶段的指标守卫；排空超时或任务取消时仍记录 Canceled，不产生业务收件事件。

use std::{sync::Arc, time::Instant};

use auction_application::external_metrics::{
    ExternalMetricsEvent, ExternalMetricsObserver, MailMaintenanceOutcome, MailWorld,
};
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

/// 维护轮次的进程内快照观察；只在真实执行维护时存在，不改变调度或完成判定。
/// 未显式结束（超时取消、panic 收口）按 `Canceled` 记录，与轮询指标的取消口径一致。
pub(super) struct MaintenanceObservation {
    observer: Arc<dyn ExternalMetricsObserver>,
    world: MailWorld,
    outcome: MailMaintenanceOutcome,
}

impl MaintenanceObservation {
    pub(super) fn begin(observer: Arc<dyn ExternalMetricsObserver>, world: MailWorld) -> Self {
        observer.observe(ExternalMetricsEvent::MailMaintenanceStarted { world });
        Self { observer, world, outcome: MailMaintenanceOutcome::Canceled }
    }

    pub(super) fn finish(mut self, result: WorkerResult) {
        self.outcome = match result {
            WorkerResult::Success => MailMaintenanceOutcome::Succeeded,
            WorkerResult::Error => MailMaintenanceOutcome::Failed,
            WorkerResult::Canceled => MailMaintenanceOutcome::Canceled,
        };
    }
}

impl Drop for MaintenanceObservation {
    fn drop(&mut self) {
        self.observer.observe(ExternalMetricsEvent::MailMaintenanceFinished {
            world: self.world,
            outcome: self.outcome,
        });
    }
}

/// 把 Worker 的 world 字符串映射到快照闭集；未知 world 不产生观察。
pub(super) fn mail_world(world_id: &str) -> Option<MailWorld> {
    match world_id {
        "main" => Some(MailWorld::Main),
        "isekai" => Some(MailWorld::Isekai),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use auction_application::external_metrics::{
        ExternalMetricsEvent, ExternalMetricsObserver, MailMaintenanceOutcome, MailWorld,
    };
    use auction_observability::WorkerResult;

    use super::MaintenanceObservation;

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<ExternalMetricsEvent>>);

    impl ExternalMetricsObserver for RecordingObserver {
        fn observe(&self, event: ExternalMetricsEvent) {
            self.0.lock().expect("lock").push(event);
        }
    }

    fn begin(observer: &Arc<RecordingObserver>) -> MaintenanceObservation {
        MaintenanceObservation::begin(
            Arc::clone(observer) as Arc<dyn ExternalMetricsObserver>,
            MailWorld::Main,
        )
    }

    fn recorded_outcome(observer: &RecordingObserver) -> MailMaintenanceOutcome {
        let events = observer.0.lock().expect("lock");
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            ExternalMetricsEvent::MailMaintenanceStarted { world: MailWorld::Main }
        ));
        match events[1] {
            ExternalMetricsEvent::MailMaintenanceFinished { world: MailWorld::Main, outcome } => {
                outcome
            }
            _ => panic!("维护结束事件缺失"),
        }
    }

    #[test]
    fn finish_maps_worker_result_to_observed_outcome() {
        for (result, expected) in [
            (WorkerResult::Success, MailMaintenanceOutcome::Succeeded),
            (WorkerResult::Error, MailMaintenanceOutcome::Failed),
            (WorkerResult::Canceled, MailMaintenanceOutcome::Canceled),
        ] {
            let observer = Arc::new(RecordingObserver::default());
            begin(&observer).finish(result);
            assert_eq!(recorded_outcome(&observer), expected);
        }
    }

    #[test]
    fn dropping_without_finish_records_canceled() {
        let observer = Arc::new(RecordingObserver::default());
        drop(begin(&observer));
        assert_eq!(recorded_outcome(&observer), MailMaintenanceOutcome::Canceled);
    }
}
