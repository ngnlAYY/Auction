//! Worker 循环与重试指标的 Prometheus 适配器。

use std::sync::Arc;

use prometheus::{
    CounterVec, GaugeVec, HistogramOpts, HistogramVec, Opts, Registry, core::Collector,
};

use super::types::{
    BusinessEvent, BusinessOutcome, BusinessRecorder, MetricsError, WorkerLoop, WorkerRecorder,
    WorkerResult, WorkerRetryKind,
};

/// Prometheus Worker 指标的固定 label 适配器。
pub(super) struct PrometheusWorkerRecorder {
    iterations: CounterVec,
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
        let durations = HistogramVec::new(
            HistogramOpts::new(
                "auction_worker_loop_iteration_duration_seconds",
                "Worker loop iteration duration in seconds.",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0, 30.0]),
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

    fn observe_business(&self, event: BusinessEvent, outcome: BusinessOutcome) {
        self.business.observe(event, outcome);
    }

    fn observe_business_duration(&self, event: BusinessEvent, duration: std::time::Duration) {
        self.business.observe_duration(event, duration);
    }
}

fn business_event_for_worker(loop_kind: WorkerLoop) -> Option<BusinessEvent> {
    match loop_kind {
        WorkerLoop::MailMain | WorkerLoop::MailIsekai => Some(BusinessEvent::MailIntake),
        WorkerLoop::Delivery => Some(BusinessEvent::MailDelivery),
        WorkerLoop::Auction => Some(BusinessEvent::AuctionLifecycle),
        WorkerLoop::AuctionFulfillment => Some(BusinessEvent::Fulfillment),
        WorkerLoop::AuctionRecovery => Some(BusinessEvent::Recovery),
        WorkerLoop::OperationsAlertScan
        | WorkerLoop::OperationsAlertDispatch
        | WorkerLoop::OperationsAlertReconcile => Some(BusinessEvent::OperationsAlert),
    }
}

fn valid_retry_mode(loop_kind: WorkerLoop, retry_kind: WorkerRetryKind) -> bool {
    match loop_kind {
        WorkerLoop::MailMain | WorkerLoop::MailIsekai => true,
        WorkerLoop::Delivery | WorkerLoop::AuctionFulfillment | WorkerLoop::AuctionRecovery => {
            matches!(retry_kind, WorkerRetryKind::Persisted)
        }
        WorkerLoop::Auction
        | WorkerLoop::OperationsAlertScan
        | WorkerLoop::OperationsAlertDispatch
        | WorkerLoop::OperationsAlertReconcile => false,
    }
}
