//! 固定业务事件的 Prometheus 适配器。

use std::sync::Arc;

use prometheus::{CounterVec, HistogramOpts, HistogramVec, Opts, Registry};

use super::types::{BusinessEvent, BusinessOutcome, BusinessRecorder, MetricsError};

/// Prometheus 业务指标的固定 label 适配器。
pub(super) struct PrometheusBusinessRecorder {
    events: CounterVec,
    durations: HistogramVec,
}

impl PrometheusBusinessRecorder {
    pub(super) fn build(registry: &Registry) -> Result<Arc<dyn BusinessRecorder>, MetricsError> {
        let events = CounterVec::new(
            Opts::new(
                "auction_business_events_total",
                "Total business events by fixed event and outcome.",
            ),
            &["event", "outcome"],
        )
        .map_err(MetricsError::Registry)?;
        let durations = HistogramVec::new(
            HistogramOpts::new(
                "auction_business_event_duration_seconds",
                "Business event processing duration in seconds.",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 30.0]),
            &["event"],
        )
        .map_err(MetricsError::Registry)?;

        // 预先建立闭集的零值，确保 scrape 在业务尚未发生时也能看到完整指标族。
        for event in BusinessEvent::ALL {
            durations.with_label_values(&[event.label()]);
            for outcome in BusinessOutcome::ALL {
                events.with_label_values(&[event.label(), outcome.label()]);
            }
        }
        registry.register(Box::new(events.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(durations.clone())).map_err(MetricsError::Registry)?;

        Ok(Arc::new(Self { events, durations }))
    }
}

impl BusinessRecorder for PrometheusBusinessRecorder {
    fn observe(&self, event: BusinessEvent, outcome: BusinessOutcome) {
        self.events.with_label_values(&[event.label(), outcome.label()]).inc();
    }

    fn observe_duration(&self, event: BusinessEvent, duration: std::time::Duration) {
        self.durations.with_label_values(&[event.label()]).observe(duration.as_secs_f64());
    }
}
