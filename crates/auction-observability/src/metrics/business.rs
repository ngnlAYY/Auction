//! 固定业务事件的 Prometheus 适配器。

use std::sync::Arc;

use prometheus::{Counter, CounterVec, Histogram, HistogramOpts, HistogramVec, Opts, Registry};

use super::types::{BusinessEvent, BusinessOutcome, BusinessRecorder, MetricsError};

/// Prometheus 业务指标的固定 label 适配器。
pub(super) struct PrometheusBusinessRecorder {
    events: [[Counter; BusinessOutcome::ALL.len()]; BusinessEvent::ALL.len()],
    durations: [Histogram; BusinessEvent::ALL.len()],
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

        // 保留完整闭集零值并缓存共享句柄；逐笔记录不再重复 label 哈希、读锁和 Arc 克隆。
        let event_handles = BusinessEvent::ALL.map(|event| {
            BusinessOutcome::ALL
                .map(|outcome| events.with_label_values(&[event.label(), outcome.label()]))
        });
        let duration_handles =
            BusinessEvent::ALL.map(|event| durations.with_label_values(&[event.label()]));
        registry.register(Box::new(events)).map_err(MetricsError::Registry)?;
        registry.register(Box::new(durations)).map_err(MetricsError::Registry)?;

        Ok(Arc::new(Self { events: event_handles, durations: duration_handles }))
    }
}

impl BusinessRecorder for PrometheusBusinessRecorder {
    fn observe(&self, event: BusinessEvent, outcome: BusinessOutcome) {
        self.events[event as usize][outcome as usize].inc();
    }

    fn observe_duration(&self, event: BusinessEvent, duration: std::time::Duration) {
        self.durations[event as usize].observe(duration.as_secs_f64());
    }
}

// 缓存按枚举判别值寻址；闭集顺序在编译期核对，新增或重排变体时不能静默串写指标。
const _: () = {
    let mut index = 0;
    while index < BusinessEvent::ALL.len() {
        assert!(BusinessEvent::ALL[index] as usize == index);
        index += 1;
    }
    index = 0;
    while index < BusinessOutcome::ALL.len() {
        assert!(BusinessOutcome::ALL[index] as usize == index);
        index += 1;
    }
};
