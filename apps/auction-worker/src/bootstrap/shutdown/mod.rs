//! Worker 排空协议的装配桥接；不改写持久状态或业务授权。
use std::{
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use auction_application::operations::{
    CriticalWriteKind, ExternalWriteActivity, ExternalWriteActivityFactory,
    ExternalWriteActivityPort, ExternalWriteLifecycle,
};
use auction_observability::{
    ShutdownCriticalKind, ShutdownOutcome, ShutdownPhase, ShutdownReason, ShutdownRecorder,
    ShutdownRole,
};
use auction_process::{
    DrainActivity, DrainActivityObserver, DrainCoordinator, DrainOutcome, TerminationSignals,
    wait_for_drain,
};

struct Activity(DrainActivity);
impl ExternalWriteActivityPort for Activity {
    fn enter_critical(&self) -> bool {
        self.0.enter_critical()
    }
    fn complete(&self) {
        self.0.complete();
    }
    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    fn poll_cancelled(&self, context: &mut Context<'_>) -> Poll<()> {
        self.0.poll_cancelled(context)
    }
}
struct Observer {
    kind: ShutdownCriticalKind,
    metrics: Arc<dyn ShutdownRecorder>,
}
impl DrainActivityObserver for Observer {
    fn entered(&self) {
        self.metrics.critical_entered(self.kind);
    }
    fn released(&self, persisted: bool) {
        self.metrics.critical_released(self.kind);
        if !persisted {
            tracing::warn!(kind = self.kind.label(), "外部写入活动未确认本地结果；保留原持久围栏");
        }
    }
}
struct Factory {
    coordinator: DrainCoordinator,
    metrics: Arc<dyn ShutdownRecorder>,
}
impl ExternalWriteActivityFactory for Factory {
    fn activity(&self, kind: CriticalWriteKind) -> ExternalWriteActivity {
        let kind = match kind {
            CriticalWriteKind::Background => ShutdownCriticalKind::Background,
            CriticalWriteKind::DeliverySend => ShutdownCriticalKind::DeliverySend,
            CriticalWriteKind::DeliveryTake => ShutdownCriticalKind::DeliveryTake,
            CriticalWriteKind::DeliveryReturn => ShutdownCriticalKind::DeliveryReturn,
            CriticalWriteKind::ComposeDiscard => ShutdownCriticalKind::ComposeDiscard,
            CriticalWriteKind::Forum => ShutdownCriticalKind::Forum,
            CriticalWriteKind::ForumPm => ShutdownCriticalKind::ForumPm,
        };
        ExternalWriteActivity::new(Arc::new(Activity(
            self.coordinator
                .activity(Arc::new(Observer { kind, metrics: Arc::clone(&self.metrics) })),
        )))
    }
}

pub(super) struct WorkerShutdown {
    coordinator: DrainCoordinator,
    pub(super) lifecycle: ExternalWriteLifecycle,
    metrics: Arc<dyn ShutdownRecorder>,
    critical: Duration,
    ordinary: Duration,
}
impl WorkerShutdown {
    pub(super) fn new(
        settings: auction_config::ShutdownSettings,
        metrics: Arc<dyn ShutdownRecorder>,
    ) -> Self {
        let coordinator = DrainCoordinator::default();
        let lifecycle = if settings.critical_write_timeout_seconds == 0 {
            ExternalWriteLifecycle::default()
        } else {
            ExternalWriteLifecycle::new(Arc::new(Factory {
                coordinator: coordinator.clone(),
                metrics: Arc::clone(&metrics),
            }))
        };
        Self {
            coordinator,
            lifecycle,
            metrics,
            critical: Duration::from_secs(settings.critical_write_timeout_seconds),
            ordinary: Duration::from_secs(settings.drain_timeout_seconds),
        }
    }
    pub(super) fn close(&self) {
        self.coordinator.close();
    }
    /// 保持原 Worker future 的所有权；等待期间仍 poll runner、PM、续租及凭据校验。
    pub(super) async fn drain<F>(
        &self,
        signals: &mut TerminationSignals,
        worker: F,
        reason: ShutdownReason,
        forced: &mut bool,
    ) -> anyhow::Result<()>
    where
        F: std::future::Future<Output = anyhow::Result<()>>,
    {
        tokio::pin!(worker);
        if !self.critical.is_zero() {
            let started = Instant::now();
            let critical = async {
                tokio::select! {
                    biased;
                    result = &mut worker => Some(result),
                    () = self.coordinator.wait_critical() => None,
                }
            };
            let result = wait_for_drain(signals, self.critical, critical).await;
            self.metrics.phase_finished(
                ShutdownRole::Worker,
                ShutdownPhase::Critical,
                started.elapsed(),
            );
            match result {
                Ok(DrainOutcome::Completed(None)) => {}
                Ok(DrainOutcome::Completed(Some(result))) => {
                    self.finished(
                        reason,
                        if result.is_ok() {
                            ShutdownOutcome::Completed
                        } else {
                            ShutdownOutcome::Failed
                        },
                    );
                    return result;
                }
                other => {
                    return self.force(
                        other.map(|outcome| match outcome {
                            DrainOutcome::Completed(_) => DrainOutcome::Completed(Ok(())),
                            DrainOutcome::TimedOut => DrainOutcome::TimedOut,
                            DrainOutcome::Interrupted(signal) => DrainOutcome::Interrupted(signal),
                        }),
                        reason,
                        forced,
                    );
                }
            }
        }
        let started = Instant::now();
        let result = wait_for_drain(signals, self.ordinary, &mut worker).await;
        self.metrics.phase_finished(ShutdownRole::Worker, ShutdownPhase::Drain, started.elapsed());
        match result {
            Ok(DrainOutcome::Completed(result)) => {
                self.finished(
                    reason,
                    if result.is_ok() {
                        ShutdownOutcome::Completed
                    } else {
                        ShutdownOutcome::Failed
                    },
                );
                result
            }
            other => self.force(other, reason, forced),
        }
    }
    fn finished(&self, reason: ShutdownReason, outcome: ShutdownOutcome) {
        self.metrics.finished(ShutdownRole::Worker, reason, outcome);
        tracing::info!(reason = reason.label(), outcome = outcome.label(), "worker 排空结束");
    }
    fn force(
        &self,
        result: std::io::Result<DrainOutcome<anyhow::Result<()>>>,
        reason: ShutdownReason,
        forced: &mut bool,
    ) -> anyhow::Result<()> {
        *forced = true;
        let (outcome, detail) = match result {
            Ok(DrainOutcome::TimedOut) => {
                (ShutdownOutcome::TimedOut, "worker 排空预算耗尽".to_owned())
            }
            Ok(DrainOutcome::Interrupted(signal)) => {
                (ShutdownOutcome::Interrupted, format!("worker 排空被 {signal} 强制中断"))
            }
            Err(error) => (ShutdownOutcome::Failed, format!("worker 排空信号失败：{error}")),
            Ok(DrainOutcome::Completed(result)) => return result,
        };
        self.finished(reason, outcome);
        Err(anyhow::anyhow!(detail))
    }
}

#[cfg(test)]
mod tests;
