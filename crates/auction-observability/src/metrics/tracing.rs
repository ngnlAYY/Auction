//! tracing 健康状态的 Prometheus collector。

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use prometheus::{
    Gauge, IntCounter, Opts,
    core::{Collector, Desc},
    proto::MetricFamily,
};

use super::types::MetricsError;
use crate::health::{TracingHealth, TracingHealthReader, TracingStatus};

#[derive(Debug)]
pub(super) struct DisabledTracingHealth;

impl TracingHealthReader for DisabledTracingHealth {
    fn snapshot(&self) -> TracingHealth {
        TracingHealth {
            status: TracingStatus::Disabled,
            dropped_span_count: 0,
            export_failure_count: 0,
            last_success_at: None,
            last_failure_at: None,
            last_error_code: String::new(),
        }
    }
}

/// 旧版 tracing collector 的低基数 Rust 实现。
/// counter 在读取时只向前同步到心跳安全快照，避免把不可信的负数或回退值写入 Prometheus。
pub(super) struct TracingCollector {
    reader: Arc<dyn TracingHealthReader>,
    enabled: Gauge,
    degraded: Gauge,
    dropped_spans: IntCounter,
    export_failures: IntCounter,
    last_dropped_spans: AtomicU64,
    last_export_failures: AtomicU64,
    last_success: Gauge,
    last_failure: Gauge,
}

impl TracingCollector {
    pub(super) fn new(reader: Arc<dyn TracingHealthReader>) -> Result<Self, MetricsError> {
        Ok(Self {
            reader,
            enabled: Gauge::with_opts(Opts::new(
                "auction_tracing_enabled",
                "Whether process tracing is enabled.",
            ))
            .map_err(MetricsError::Registry)?,
            degraded: Gauge::with_opts(Opts::new(
                "auction_tracing_export_degraded",
                "Whether tracing export is degraded.",
            ))
            .map_err(MetricsError::Registry)?,
            dropped_spans: IntCounter::with_opts(Opts::new(
                "auction_tracing_dropped_spans_total",
                "Total spans dropped by the bounded tracing processor.",
            ))
            .map_err(MetricsError::Registry)?,
            export_failures: IntCounter::with_opts(Opts::new(
                "auction_tracing_export_failures_total",
                "Total tracing export failures.",
            ))
            .map_err(MetricsError::Registry)?,
            last_dropped_spans: AtomicU64::new(0),
            last_export_failures: AtomicU64::new(0),
            last_success: Gauge::with_opts(Opts::new(
                "auction_tracing_export_last_success_timestamp_seconds",
                "Unix timestamp of the last successful tracing export, or zero.",
            ))
            .map_err(MetricsError::Registry)?,
            last_failure: Gauge::with_opts(Opts::new(
                "auction_tracing_export_last_failure_timestamp_seconds",
                "Unix timestamp of the last failed tracing export, or zero.",
            ))
            .map_err(MetricsError::Registry)?,
        })
    }

    fn sync_counter(counter: &IntCounter, last: &AtomicU64, value: i64) {
        let target = u64::try_from(value).unwrap_or(0);
        // 并行 scrape 原子领取各自的增量，避免同时读取旧 counter 后重复累计同一批失败。
        let previous = last.fetch_max(target, Ordering::Relaxed);
        if target > previous {
            counter.inc_by(target - previous);
        }
    }

    fn timestamp_seconds(value: Option<chrono::DateTime<chrono::Utc>>) -> f64 {
        value.map_or(0.0, |timestamp| timestamp.timestamp_micros() as f64 / 1_000_000.0)
    }
}

impl Collector for TracingCollector {
    fn desc(&self) -> Vec<&Desc> {
        [
            self.enabled.desc(),
            self.degraded.desc(),
            self.dropped_spans.desc(),
            self.export_failures.desc(),
            self.last_success.desc(),
            self.last_failure.desc(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let health = self.reader.snapshot();
        self.enabled.set(f64::from(!matches!(health.status, TracingStatus::Disabled)));
        self.degraded.set(f64::from(health.status == TracingStatus::Degraded));
        Self::sync_counter(
            &self.dropped_spans,
            &self.last_dropped_spans,
            health.dropped_span_count,
        );
        Self::sync_counter(
            &self.export_failures,
            &self.last_export_failures,
            health.export_failure_count,
        );
        self.last_success.set(Self::timestamp_seconds(health.last_success_at));
        self.last_failure.set(Self::timestamp_seconds(health.last_failure_at));

        [
            self.enabled.collect(),
            self.degraded.collect(),
            self.dropped_spans.collect(),
            self.export_failures.collect(),
            self.last_success.collect(),
            self.last_failure.collect(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

pub(super) fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_graphic() && byte != b'"')
}
