//! Worker 循环与重试指标的 Prometheus 适配器。

use std::sync::Arc;

use auction_application::core::diagnostics::SafeErrorKind;
use prometheus::{
    CounterVec, GaugeVec, HistogramOpts, HistogramVec, Opts, Registry, core::Collector,
};

use super::types::{
    BusinessEvent, BusinessOutcome, BusinessRecorder, MetricsError, WorkerLoop, WorkerPanicOwner,
    WorkerRecorder, WorkerResult, WorkerRetryKind,
};

/// Prometheus Worker 指标的固定 label 适配器。
pub(super) struct PrometheusWorkerRecorder {
    iterations: CounterVec,
    unpersisted_errors: CounterVec,
    task_panics: CounterVec,
    durations: HistogramVec,
    batch_items: CounterVec,
    batch_capacity: CounterVec,
    batch_saturated: CounterVec,
    retry_modes: GaugeVec,
    retries: CounterVec,
    schedule_delay: HistogramVec,
    business: Arc<dyn BusinessRecorder>,
}

impl PrometheusWorkerRecorder {
    pub(super) fn build(
        registry: &Registry,
        business: Arc<dyn BusinessRecorder>,
    ) -> Result<Arc<dyn WorkerRecorder>, MetricsError> {
        let iterations = CounterVec::new(
            Opts::new(
                "auction_worker_loop_iterations_total",
                "Total Worker loop iterations by fixed loop and result.",
            ),
            &["loop", "result"],
        )
        .map_err(MetricsError::Registry)?;
        let unpersisted_errors = CounterVec::new(
            Opts::new(
                "auction_worker_unpersisted_error_total",
                "Worker failures without a confirmed persisted disposition.",
            ),
            &["loop", "kind"],
        )
        .map_err(MetricsError::Registry)?;
        let task_panics = CounterVec::new(
            Opts::new(
                "auction_worker_task_panics_total",
                "Worker item task panics by fixed loop and owner.",
            ),
            &["loop", "owner"],
        )
        .map_err(MetricsError::Registry)?;
        for loop_kind in WorkerLoop::ALL {
            for owner in WorkerPanicOwner::ALL {
                let _ = task_panics.with_label_values(&[loop_kind.label(), owner.label()]);
            }
        }
        let durations = HistogramVec::new(
            HistogramOpts::new(
                "auction_worker_loop_iteration_duration_seconds",
                "Worker loop iteration duration in seconds.",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0, 30.0, 60.0, 120.0, 300.0]),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;
        let batch_items = CounterVec::new(
            Opts::new("auction_worker_batch_items_total", "Total successful Worker batch items."),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;
        let batch_capacity = CounterVec::new(
            Opts::new(
                "auction_worker_batch_capacity_total",
                "Total configured capacity of successful Worker batches.",
            ),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;
        let batch_saturated = CounterVec::new(
            Opts::new(
                "auction_worker_batch_saturated_total",
                "Total successful Worker batches that reached capacity.",
            ),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;
        let retry_modes = GaugeVec::new(
            Opts::new(
                "auction_worker_retry_mode_bound",
                "Whether a concrete Worker loop bound the supported retry mode.",
            ),
            &["loop", "kind"],
        )
        .map_err(MetricsError::Registry)?;
        let retries = CounterVec::new(
            Opts::new(
                "auction_worker_retry_attempts_total",
                "Total Worker retry attempts by fixed loop and retry kind.",
            ),
            &["loop", "kind"],
        )
        .map_err(MetricsError::Registry)?;
        let schedule_delay = HistogramVec::new(
            HistogramOpts::new(
                "auction_worker_schedule_delay_seconds",
                "Worker scheduled-to-actual start delay in seconds.",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;

        for collector in [
            Box::new(iterations.clone()) as Box<dyn Collector>,
            Box::new(unpersisted_errors.clone()),
            Box::new(task_panics.clone()),
            Box::new(durations.clone()),
            Box::new(batch_items.clone()),
            Box::new(batch_capacity.clone()),
            Box::new(batch_saturated.clone()),
            Box::new(retry_modes.clone()),
            Box::new(retries.clone()),
            Box::new(schedule_delay.clone()),
        ] {
            registry.register(collector).map_err(MetricsError::Registry)?;
        }

        Ok(Arc::new(Self {
            iterations,
            unpersisted_errors,
            task_panics,
            durations,
            batch_items,
            batch_capacity,
            batch_saturated,
            retry_modes,
            retries,
            schedule_delay,
            business,
        }))
    }
}

impl WorkerRecorder for PrometheusWorkerRecorder {
    fn observe_task_panic(&self, loop_kind: WorkerLoop, owner: WorkerPanicOwner) {
        self.task_panics.with_label_values(&[loop_kind.label(), owner.label()]).inc();
    }

    fn observe_unpersisted_error(&self, loop_kind: WorkerLoop, kind: SafeErrorKind) {
        self.unpersisted_errors.with_label_values(&[loop_kind.label(), kind.label()]).inc();
    }

    fn bind_retry_mode(&self, loop_kind: WorkerLoop, retry_kind: WorkerRetryKind) {
        if !valid_retry_mode(loop_kind, retry_kind) {
            return;
        }
        self.retry_modes.with_label_values(&[loop_kind.label(), retry_kind.label()]).set(1.0);
    }

    fn observe_iteration(
        &self,
        loop_kind: WorkerLoop,
        result: WorkerResult,
        duration: std::time::Duration,
    ) {
        self.iterations.with_label_values(&[loop_kind.label(), result.label()]).inc();
        self.durations.with_label_values(&[loop_kind.label()]).observe(duration.as_secs_f64());
        if let Some(event) = business_event_for_worker(loop_kind) {
            let outcome = match result {
                WorkerResult::Success => BusinessOutcome::Success,
                WorkerResult::Error | WorkerResult::Canceled => BusinessOutcome::Error,
            };
            self.business.observe(event, outcome);
            self.business.observe_duration(event, duration);
        }
    }

    fn observe_batch(&self, loop_kind: WorkerLoop, items: usize, capacity: usize) {
        if capacity == 0 || items > capacity {
            return;
        }
        let label = loop_kind.label();
        self.batch_items.with_label_values(&[label]).inc_by(items as f64);
        self.batch_capacity.with_label_values(&[label]).inc_by(capacity as f64);
        if items == capacity {
            self.batch_saturated.with_label_values(&[label]).inc();
        }
    }

    fn observe_retry(&self, loop_kind: WorkerLoop, retry_kind: WorkerRetryKind) {
        if !valid_retry_mode(loop_kind, retry_kind) {
            return;
        }
        self.bind_retry_mode(loop_kind, retry_kind);
        self.retries.with_label_values(&[loop_kind.label(), retry_kind.label()]).inc();
    }

    fn observe_schedule_delay(&self, loop_kind: WorkerLoop, delay: std::time::Duration) {
        self.schedule_delay.with_label_values(&[loop_kind.label()]).observe(delay.as_secs_f64());
    }
}

fn business_event_for_worker(loop_kind: WorkerLoop) -> Option<BusinessEvent> {
    let event = match loop_kind {
        // 维护和纯观察只计阶段轮次；不能因循环拆分重复增加 MailIntake 业务事件。
        WorkerLoop::MailMaintenanceMain
        | WorkerLoop::MailMaintenanceIsekai
        | WorkerLoop::MailReadObservationMain
        | WorkerLoop::MailReadObservationIsekai
        | WorkerLoop::PlayerNotifications => return None,
        WorkerLoop::MailMain | WorkerLoop::MailIsekai => BusinessEvent::MailIntake,
        WorkerLoop::Delivery => BusinessEvent::MailDelivery,
        WorkerLoop::Auction => BusinessEvent::AuctionLifecycle,
        WorkerLoop::Settlement => BusinessEvent::Settlement,
        WorkerLoop::AuctionFulfillment => BusinessEvent::Fulfillment,
        WorkerLoop::AuctionRecovery => BusinessEvent::Recovery,
        WorkerLoop::Forum => BusinessEvent::ForumSync,
        WorkerLoop::OperationsAlertScan
        | WorkerLoop::OperationsAlertDispatch
        | WorkerLoop::OperationsAlertReconcile => BusinessEvent::OperationsAlert,
    };
    Some(event)
}

fn valid_retry_mode(loop_kind: WorkerLoop, retry_kind: WorkerRetryKind) -> bool {
    match loop_kind {
        WorkerLoop::MailMain | WorkerLoop::MailIsekai => true,
        WorkerLoop::Delivery | WorkerLoop::AuctionFulfillment | WorkerLoop::AuctionRecovery => {
            matches!(retry_kind, WorkerRetryKind::Persisted)
        }
        WorkerLoop::Auction
        | WorkerLoop::Settlement
        | WorkerLoop::MailMaintenanceMain
        | WorkerLoop::MailMaintenanceIsekai
        | WorkerLoop::MailReadObservationMain
        | WorkerLoop::MailReadObservationIsekai
        | WorkerLoop::Forum
        | WorkerLoop::OperationsAlertScan
        | WorkerLoop::OperationsAlertDispatch
        | WorkerLoop::OperationsAlertReconcile
        | WorkerLoop::PlayerNotifications => false,
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::metrics::worker as parent;

    #[test]
    fn task_panics_are_preinitialized_and_split_by_owner() {
        let registry = Registry::new();
        let recorder = PrometheusWorkerRecorder::build(
            &registry,
            super::super::types::noop_business_recorder(),
        )
        .unwrap();
        recorder.observe_task_panic(WorkerLoop::Auction, WorkerPanicOwner::Settlement);
        let families = registry.gather();
        let family = families
            .iter()
            .find(|family| family.name() == "auction_worker_task_panics_total")
            .unwrap();
        assert_eq!(family.get_metric().len(), WorkerLoop::ALL.len() * WorkerPanicOwner::ALL.len());
        for metric in family.get_metric() {
            let labels = metric.get_label();
            let value = |name| labels.iter().find(|label| label.name() == name).unwrap().value();
            let expected = if value("loop") == "auction" && value("owner") == "settlement" {
                1.0
            } else {
                0.0
            };
            assert_eq!(metric.get_counter().get_value(), expected);
        }
    }
}
